//! The Monitor tab: the server's strip, the views' strip, what the account cannot see, then the
//! Overview (charts and the longest running sessions) or the Sessions (the list, its details,
//! the list of actions sent from the tab).
//!
//! A frame draws from the tab's latest read: the list was sorted and filtered when it came
//! (or when the sort or the filter changed), so a frame only looks at the rows on screen,
//! however many sessions the server has.

use crate::app::action::{Action, MonitorAction};
use crate::app::monitor::statement::{Layout, Shown};
use crate::app::monitor::{METRICS, Metric, MonState, MonitorHits, MonitorTab, MonitorView, SortKey};
use crate::app::monitor::{query_age, tx_age};
use crate::app::{App, Focus};
use crate::keymap::Ctx;
use crate::text::{clip, width, wrap};
use crate::theme;
use crate::widgets::{panel, put, sparkline};
use datarig_core::i18n::{I18n, Label, Msg};
use datarig_core::monitor::locks::{Blame, Holder, LockObject};
use datarig_core::monitor::{
    CounterKind, ExtraKey, FullText, GaugeKind, Hidden, MAX_WAITERS, MonitorFormat, Need, Part, Proof, SessionKind,
    SessionRow, SessionState, Timestamp, Tx, Visibility, WaitClass,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

mod lines;
mod replication;

thread_local! {
    /// What the frames drew since the last [`take_work`]: rows of sessions, chart tiles and
    /// lines of details (the bench's measure that a frame's work does not grow with the
    /// sessions a server has).
    static WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The work counted since the last call (see [`WORK`]), and start again.
pub fn take_work() -> u64 {
    WORK.with(|w| w.replace(0))
}

fn work(n: u64) {
    WORK.with(|w| w.set(w.get() + n));
}

/// The narrowest a chart tile is.
const TILE_MIN: u16 = 26;

/// The narrowest a chart of several series is: two side by side need twice this.
const CHART_MIN: u16 = 40;

/// The least room a chart's legend keeps in the compact list (names longer than leave it are cut).
const COMPACT_LEGEND_MIN: usize = 28;

/// The rows of a chart's lines.
const PLOT_ROWS: u16 = 5;

/// The most rows a chart's legend takes (it wraps rather than cutting a series).
const LEGEND_ROWS: usize = 2;

/// A chart of the Overview: its title and its series, in the order of their colours.
struct ChartDef {
    title: Label,
    series: &'static [Metric],
    /// A series without which the chart is not there at all (the rows by operation: none
    /// with `track_counts` off, and the rows read alone are not that chart).
    needs: Option<Metric>,
    /// Rates per second (else counts as they stand).
    rates: bool,
    /// A series drawn only while it is above zero somewhere in what is shown.
    only_above_zero: Option<Metric>,
    /// Counted from the process list, which holds the account's own threads only without
    /// `PROCESS`: the title says so then.
    own_account: bool,
}

const STATEMENTS_BY_TYPE: ChartDef = ChartDef {
    title: Label::MonitorChartStatementsByType,
    series: &[
        Metric::Rate(CounterKind::Selects),
        Metric::Rate(CounterKind::Inserts),
        Metric::Rate(CounterKind::Updates),
        Metric::Rate(CounterKind::Deletes),
    ],
    needs: None,
    rates: true,
    only_above_zero: None,
    own_account: false,
};

const ROWS_BY_OPERATION: ChartDef = ChartDef {
    title: Label::MonitorChartRowsByOperation,
    series: &[
        Metric::Rate(CounterKind::RowsRead),
        Metric::Rate(CounterKind::RowsInserted),
        Metric::Rate(CounterKind::RowsUpdated),
        Metric::Rate(CounterKind::RowsDeleted),
    ],
    needs: Some(Metric::Rate(CounterKind::RowsInserted)),
    rates: true,
    only_above_zero: None,
    own_account: false,
};

const SESSION_STATES: ChartDef = ChartDef {
    title: Label::MonitorChartSessionsByState,
    series: &[
        Metric::Gauge(GaugeKind::StateRunning),
        Metric::Gauge(GaugeKind::StateWaiting),
        Metric::Gauge(GaugeKind::StateIdleInTx),
        Metric::Gauge(GaugeKind::StateIdle),
        Metric::Gauge(GaugeKind::StateBackground),
        Metric::Gauge(GaugeKind::StateUnknown),
    ],
    needs: None,
    rates: false,
    only_above_zero: Some(Metric::Gauge(GaugeKind::StateUnknown)),
    own_account: false,
};

const THREAD_COMMANDS: ChartDef = ChartDef {
    title: Label::MonitorChartSessionsByState,
    series: &[
        Metric::Gauge(GaugeKind::CommandQuery),
        Metric::Gauge(GaugeKind::CommandSleep),
        Metric::Gauge(GaugeKind::CommandDaemon),
        Metric::Gauge(GaugeKind::CommandConnect),
        Metric::Gauge(GaugeKind::CommandBinlogDump),
        Metric::Gauge(GaugeKind::CommandOther),
    ],
    needs: None,
    rates: false,
    only_above_zero: None,
    own_account: true,
};

/// A chart as a frame draws it.
struct ChartData {
    title: String,
    /// The sum of its own series' latest values (what is drawn), `–` when none has one.
    total: String,
    series: Vec<Drawn>,
    rates: bool,
    /// Why the chart is not there, said in its place.
    missing: Option<String>,
}

/// A series of a chart: its name, its values (oldest first, `None` a gap) and its colour's
/// place in the theme's chart colours.
struct Drawn {
    name: String,
    values: Vec<Option<f64>>,
    last: Option<f64>,
    colour: usize,
}

impl ChartData {
    /// The legend's entries: each series' name and latest value.
    fn legend(&self) -> Vec<String> {
        self.series.iter().map(|s| format!("{} {}", s.name, self.value(s.last))).collect()
    }

    fn value(&self, v: Option<f64>) -> String {
        match v {
            Some(v) if self.rates => rate(v),
            Some(v) => format!("{}", v.max(0.0) as u64),
            None => "–".into(),
        }
    }

    /// The rows a box of `w` columns takes.
    fn height(&self, w: u16) -> u16 {
        match self.missing {
            Some(_) => 3,
            None => {
                let widths = [usize::from(w.saturating_sub(2)); LEGEND_ROWS];
                2 + PLOT_ROWS + legend_rows(&self.legend(), &widths).len().max(1) as u16
            }
        }
    }
}

/// How the charts sit below the tiles.
enum ChartLayout {
    None,
    /// Side by side, the first this wide.
    Side(u16),
    /// One under the other, the first this high.
    Stacked(u16),
    /// The first (this high), the second as its compact line (or two) under it.
    FirstAndLine(u16),
}

/// Legend entries (each drawn as `■ ` and its text) in rows of the given widths, one row per
/// width: per row its entries (their index and the text shown). An entry that fits no row is
/// cut with … in the last, and those after it are left out (an index of `usize::MAX` is a lone
/// …).
fn legend_rows(items: &[String], widths: &[usize]) -> Vec<Vec<(usize, String)>> {
    let mut rows: Vec<Vec<(usize, String)>> = Vec::new();
    let (mut row, mut used) = (0usize, 0usize);
    for (i, text) in items.iter().enumerate() {
        let need = 2 + width(text);
        loop {
            let Some(&w) = widths.get(row) else { return rows };
            if rows.len() <= row {
                rows.push(Vec::new());
            }
            let gap = if rows[row].is_empty() { 0 } else { 2 };
            if used + gap + need <= w {
                rows[row].push((i, text.clone()));
                used += gap + need;
                break;
            }
            if row + 1 < widths.len() && !rows[row].is_empty() {
                row += 1;
                used = 0;
                continue;
            }
            // The last row: what is left of it, cut.
            let room = w.saturating_sub(used + gap + 2);
            if room >= 2 {
                rows[row].push((i, clip(text, room)));
            } else if w > used + gap {
                rows[row].push((usize::MAX, "…".into()));
            }
            return rows;
        }
    }
    rows
}

/// One row of a chart's legend in `at`: each entry's colour, name and latest value.
fn legend_row(c: &ChartData, row: &[(usize, String)], at: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let palette = th.chart();
    let colours = coloured(&th);
    let (mut x, end) = (at.x, at.x + at.width);
    for (n, (i, text)) in row.iter().enumerate() {
        if n > 0 {
            x += 2;
        }
        if x >= end {
            return;
        }
        if let Some(s) = c.series.get(*i) {
            let colour = if colours { palette[s.colour % palette.len()] } else { th.fg };
            x += put(buf, x, at.y, "■", 1, Style::new().fg(colour)) + 1;
        }
        if x < end {
            x += put(buf, x, at.y, text, usize::from(end - x), Style::new().fg(th.fg));
        }
    }
}

/// Whether the theme's chart colours tell series apart: none is the terminal's default and
/// no two are the same. Else a chart draws only its total, and the legend tells the series.
fn coloured(th: &theme::Theme) -> bool {
    let c = th.chart();
    !c.contains(&Color::Reset) && (1..c.len()).all(|i| !c[..i].contains(&c[i]))
}

/// A session in a transaction for longer than this (idle in it) is marked.
const LONG_TX_US: i64 = 60_000_000;

/// The most actions the action list shows (the newest).
const ACTIONS_SHOWN: usize = 6;

/// The Monitor tab in `area` (the editor's and results' place).
pub(crate) fn draw_monitor(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let id = app.tab().id;
    let focused = matches!(app.focus, Focus::Results | Focus::Inspector);
    let name = app.tab().profile.and_then(|p| app.profile(p)).map(|p| p.name.clone()).unwrap_or_default();
    let title = app.i18n.msg(&Msg::TabMonitor { name });
    let block = panel(&title, focused, area.width);
    let inner = block.inner(area);
    block.render(area, buf);
    let mut hits = MonitorHits::default();
    let read_key = app.key_for(Action::Monitor(MonitorAction::ReadNow), Ctx::Monitor);
    let info_key = app.key_for(Action::Monitor(MonitorAction::ServerInfo), Ctx::Monitor);
    let format_key = app.key_for(Action::Monitor(MonitorAction::Format), Ctx::Monitor);
    // The details' statement as shown, worked out here once per text (a frame only draws it).
    // As the profile's sessions said they read SQL (a MySQL sql_mode), not only the driver's
    // default.
    let dialect = app.tab_language(id).dialect();
    let indent = app.prefs.format_indent.spaces();
    if let Some(m) = app.monitor_mut_for_draw(id) {
        m.prepare_statement(dialect, indent);
    }
    let Some(m) = app.monitor_of(id) else { return };
    let mysql = matches!(m.format(), Some(MonitorFormat::MySql(_)));
    // The query tab whose own session the selected row is.
    let own = app.tab().profile.zip(m.selected_row()).and_then(|(p, r)| app.own_tab(p, r));
    let own = own.and_then(|t| app.tabs.get(t)).map(|t| crate::widgets::tabbar::document_name(app, t));
    let now = app.now();
    let actions = match m.actions_shown {
        true => m.actions.iter().rev().take(ACTIONS_SHOWN).map(|e| app.action_line(e, now)).collect(),
        false => Vec::new(),
    };
    let cx = Cx { i18n: &app.i18n, m, focused, mysql, read_key, format_key, own, actions };
    if inner.height == 0 || inner.width < 10 {
        return;
    }
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    let line = |y: u16| Rect { x: inner.x, y, width: inner.width, height: 1 };
    // The server, and how the tab reads it.
    cx.server_strip(line(y), buf);
    y += 1;
    if y < bottom {
        cx.view_strip(line(y), buf, &mut hits);
        y += 1;
    }
    // What the account cannot see, and a failed read: two lines each at most.
    let note_w = inner.width.saturating_sub(2) as usize;
    for (text, color) in cx.notes() {
        for l in crate::text::wrap_words(&text, note_w).iter().take(2) {
            if y + 2 >= bottom {
                break;
            }
            put(buf, inner.x + 1, y, l, note_w, Style::new().fg(color));
            y += 1;
        }
    }
    let body = Rect { x: inner.x, y, width: inner.width, height: bottom.saturating_sub(y) };
    let mut replication_top = None;
    match cx.waiting() {
        Some(text) => {
            let style = Style::new().fg(th.fg_dim);
            for (i, l) in
                wrap(&text, body.width.saturating_sub(2) as usize).iter().take(body.height as usize).enumerate()
            {
                put(buf, body.x + 1, body.y + i as u16, l, body.width.saturating_sub(2) as usize, style);
            }
        }
        None => match m.view {
            MonitorView::Overview => cx.overview(body, buf, &mut hits),
            MonitorView::Sessions => cx.sessions(body, buf, &mut hits),
            MonitorView::Locks => cx.locks(body, buf, &mut hits),
            MonitorView::Replication => replication_top = Some(cx.replication(body, buf, &mut hits)),
        },
    }
    // What the server is, in full, over the view.
    let info = m.server_info.map(|top| cx.server_info(top, &info_key, body, buf));
    if let Some((_, lines)) = info {
        hits.info_lines = lines;
    }
    let page = hits.rows.height as usize;
    if let Some(m) = app.monitor_mut_for_draw(id) {
        // The list keeps the selection on screen (from the frame's height).
        if hits.rows_view == Some(MonitorView::Sessions) && page > 0 {
            let c = m.list.cursor;
            if c < m.list.top {
                m.list.top = c;
            } else if c >= m.list.top + page {
                m.list.top = c + 1 - page;
            }
        }
        if hits.rows_view == Some(MonitorView::Locks) && page > 0 {
            m.lock_view.top = on_screen(m.lock_view.cursor, m.lock_view.top, page);
        }
        m.hits = hits;
        if let Some(top) = replication_top {
            m.replication_top = top;
        }
        if let Some((top, _)) = info {
            m.server_info = Some(top);
        }
    }
}

/// What a frame draws from.
struct Cx<'a> {
    i18n: &'a I18n,
    m: &'a MonitorTab,
    focused: bool,
    mysql: bool,
    /// The key that reads again (what a state that waits for it names).
    read_key: String,
    /// The key that shows statements laid out or as sent (what the details' note names).
    format_key: String,
    /// The name of the query tab whose own session the selected row is.
    own: Option<String>,
    /// The action list's lines, newest first (empty while it is hidden).
    actions: Vec<String>,
}

impl Cx<'_> {
    fn label(&self, l: Label) -> String {
        self.i18n.label(l).to_string()
    }

    fn msg(&self, m: Msg) -> String {
        self.i18n.msg(&m).to_string()
    }

    /// The server's version, role, uptime and connection limit, and its provider.
    fn server_strip(&self, at: Rect, buf: &mut Buffer) {
        let th = theme::cur();
        let mut parts = Vec::new();
        if let Some(s) = &self.m.server {
            parts.push(short_version(&s.version, self.mysql));
            if let Some(p) = self.m.format().and_then(|f| f.flavour().product()) {
                parts.push(p.to_string());
            }
            parts.push(self.label(if s.read_only { Label::MonitorReadOnly } else { Label::MonitorReadWrite }));
            // On the latest read's clock: the server facts are read only now and then.
            let now = self.m.snapshot.as_ref().map_or(s.read_at, |r| r.sample.time);
            if let Some(up) = s.started.and_then(|t| now.since(t)) {
                parts.push(self.msg(Msg::MonitorUptime { time: age(up) }));
            }
            if let Some(n) = s.max_connections {
                parts.push(self.msg(Msg::MonitorMaxConnections { n: n.to_string() }));
            }
        }
        let text = parts.join(" · ");
        put(
            buf,
            at.x + 1,
            at.y,
            &text,
            at.width.saturating_sub(2) as usize,
            Style::new().fg(th.fg).add_modifier(Modifier::BOLD),
        );
    }

    /// `1 Overview  2 Sessions`, then how the tab reads on the right. Where the names and how
    /// it reads do not all fit, the views not shown are named by their number only.
    fn view_strip(&self, at: Rect, buf: &mut Buffer, hits: &mut MonitorHits) {
        let th = theme::cur();
        let mut x = at.x + 1;
        let end = at.x + at.width.saturating_sub(1);
        let (state, color) = self.reading();
        let full = |i: usize, v: &MonitorView| {
            let name = match (*v, self.m.tree.waiting) {
                (MonitorView::Locks, n) if n > 0 => self.msg(Msg::MonitorViewLocksCount { n: n.to_string() }),
                _ => self.label(v.label()),
            };
            format!("{} {name}", i + 1)
        };
        let names: usize = MonitorView::ALL.iter().enumerate().map(|(i, v)| width(&full(i, v)) + 2).sum();
        let short = names + width(&state) + 1 > at.width.saturating_sub(2) as usize;
        for (i, v) in MonitorView::ALL.iter().enumerate() {
            let text = match short && *v != self.m.view {
                true => (i + 1).to_string(),
                false => full(i, v),
            };
            let w = width(&text) as u16;
            if x + w > end {
                break;
            }
            let style = match *v == self.m.view {
                true => Style::new().fg(th.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                false => Style::new().fg(th.fg_muted),
            };
            put(buf, x, at.y, &text, w as usize, style);
            hits.views.push((Rect { x, y: at.y, width: w, height: 1 }, *v));
            x += w + 2;
        }
        let w = width(&state) as u16;
        if x + w < end {
            put(buf, end - w, at.y, &state, w as usize, Style::new().fg(color));
        }
    }

    /// How the tab reads now: paused, slowed after failures, every so often.
    fn reading(&self) -> (String, Color) {
        let th = theme::cur();
        let m = self.m;
        let every = m.interval.as_str().to_string();
        match &m.state {
            MonState::Ready(_) if m.paused => (self.label(Label::MonitorPaused), th.warning),
            MonState::Ready(_) if m.delay() > m.interval.duration() => {
                let delay = format!("{}s", m.delay().as_secs());
                (self.msg(Msg::MonitorSlowed { delay, failures: m.failures.to_string() }), th.warning)
            }
            MonState::Ready(_) => (self.msg(Msg::MonitorEvery { interval: every }), th.fg_dim),
            MonState::Lost(why) => (self.msg(Msg::MonitorLost { error: why.clone() }), th.error),
            // What was read stays shown, with why nothing more comes.
            MonState::Closed => (self.msg(Msg::MonitorClosed { key: self.read_key.clone() }), th.warning),
            MonState::Idle | MonState::WaitingForProfile | MonState::Opening => {
                (self.label(Label::MonitorConnecting), th.fg_dim)
            }
            MonState::ProfileFailed(e) => {
                (self.msg(Msg::MonitorProfileFailed { error: e.clone(), key: self.read_key.clone() }), th.error)
            }
            MonState::Unavailable(e) => {
                (self.msg(Msg::MonitorUnavailable { error: e.clone(), key: self.read_key.clone() }), th.error)
            }
        }
    }

    /// What the account cannot see (and what would show it), and a failed read.
    fn notes(&self) -> Vec<(String, Color)> {
        let th = theme::cur();
        let mut v = Vec::new();
        if let Some(s) = &self.m.snapshot {
            for h in &s.hidden {
                v.push((self.hidden(h), th.warning));
            }
            // What the latest read of the locks could not see, until the next one.
            for h in self.m.locks_hidden.iter().filter(|h| !s.hidden.contains(h)) {
                v.push((self.hidden(h), th.warning));
            }
        }
        if let Some(e) = &self.m.error {
            v.push((self.msg(Msg::MonitorReadFailed { error: e.clone() }), th.error));
        }
        v
    }

    fn need(&self, need: &Need) -> String {
        match need {
            Need::Grant(g) => self.msg(Msg::MonitorNeedGrant { grant: (*g).to_string() }),
            Need::Setting(s) => self.msg(Msg::MonitorNeedSetting { setting: (*s).to_string() }),
            Need::Version(v) => self.msg(Msg::MonitorNeedVersion { version: (*v).to_string() }),
        }
    }

    fn hidden(&self, h: &Hidden) -> String {
        let need = self.need(&h.need);
        let part = match h.part {
            Part::OtherSessionDetails => Label::MonitorHiddenDetails,
            Part::OtherSessions => Label::MonitorHiddenSessions,
            Part::TransactionState => Label::MonitorHiddenTx,
            Part::UntrackedSessions => Label::MonitorHiddenUntracked,
            Part::RowLockWaits => Label::MonitorHiddenRowLockWaits,
            Part::MetadataLockWaits => Label::MonitorHiddenMetadataLockWaits,
            Part::UntrackedMetadataLocks => Label::MonitorHiddenUntrackedMetadataLocks,
            Part::ReplicationDetails => Label::MonitorHiddenReplicationDetails,
            Part::ReplicationSources => Label::MonitorHiddenReplicationSources,
            Part::Replicas => Label::MonitorHiddenReplicas,
            Part::ReplicationTimes => Label::MonitorHiddenReplicationTimes,
            Part::GroupMembers => Label::MonitorHiddenGroupMembers,
        };
        format!("{} ({need})", self.label(part))
    }

    /// What the body says while there is nothing read to show.
    fn waiting(&self) -> Option<String> {
        let m = self.m;
        if m.snapshot.is_some() {
            return None;
        }
        Some(match &m.state {
            MonState::Idle | MonState::WaitingForProfile => self.label(Label::MonitorConnecting),
            MonState::Closed => self.msg(Msg::MonitorClosed { key: self.read_key.clone() }),
            MonState::Opening => self.label(Label::MonitorOpening),
            MonState::Ready(_) if m.paused => self.label(Label::MonitorPausedEmpty),
            MonState::Ready(_) => self.label(Label::MonitorReading),
            MonState::ProfileFailed(e) => {
                self.msg(Msg::MonitorProfileFailed { error: e.clone(), key: self.read_key.clone() })
            }
            MonState::Unavailable(e) => {
                self.msg(Msg::MonitorUnavailable { error: e.clone(), key: self.read_key.clone() })
            }
            MonState::Lost(e) => self.msg(Msg::MonitorLost { error: e.clone() }),
        })
    }

    /// The metrics this server has, in their order.
    fn metrics(&self) -> Vec<Metric> {
        METRICS.into_iter().filter(|k| self.m.history_of(*k).is_some()).collect()
    }

    /// A metric's name.
    fn metric_label(&self, k: Metric) -> String {
        self.label(match k {
            Metric::Sessions => Label::MonitorMetricSessions,
            Metric::CacheHit => Label::MonitorMetricCacheHit,
            Metric::Rate(c) => match c {
                CounterKind::Commits => Label::MonitorMetricCommits,
                CounterKind::Rollbacks => Label::MonitorMetricRollbacks,
                CounterKind::ExplicitCommits => Label::MonitorMetricExplicitCommits,
                CounterKind::ExplicitRollbacks => Label::MonitorMetricExplicitRollbacks,
                CounterKind::Statements => Label::MonitorMetricStatements,
                CounterKind::RowsRead => Label::MonitorMetricRowsRead,
                CounterKind::RowsWritten => Label::MonitorMetricRowsWritten,
                CounterKind::PageRequests => Label::MonitorMetricPageRequests,
                CounterKind::PageReads => Label::MonitorMetricPageReads,
                CounterKind::BytesIn => Label::MonitorMetricBytesIn,
                CounterKind::BytesOut => Label::MonitorMetricBytesOut,
                CounterKind::Deadlocks => Label::MonitorMetricDeadlocks,
                CounterKind::RowLockWaits => Label::MonitorMetricRowLockWaits,
                CounterKind::TempBytes => Label::MonitorMetricTempBytes,
                CounterKind::TempDiskTables => Label::MonitorMetricTempDiskTables,
                CounterKind::SlowQueries => Label::MonitorMetricSlowQueries,
                CounterKind::AbortedConnects => Label::MonitorMetricAbortedConnects,
                CounterKind::Selects => Label::MonitorSeriesSelect,
                CounterKind::Inserts => Label::MonitorSeriesInsert,
                CounterKind::Updates => Label::MonitorSeriesUpdate,
                CounterKind::Deletes => Label::MonitorSeriesDelete,
                CounterKind::RowsInserted => Label::MonitorSeriesInserted,
                CounterKind::RowsUpdated => Label::MonitorSeriesUpdated,
                CounterKind::RowsDeleted => Label::MonitorSeriesDeleted,
            },
            Metric::Gauge(g) => match g {
                GaugeKind::StateRunning | GaugeKind::Active => Label::MonitorSeriesRunning,
                GaugeKind::StateWaiting | GaugeKind::WaitingForLock => Label::MonitorSeriesWaiting,
                GaugeKind::StateIdleInTx | GaugeKind::IdleInTx => Label::MonitorSeriesIdleInTx,
                GaugeKind::StateIdle | GaugeKind::Idle => Label::MonitorSeriesIdle,
                GaugeKind::StateBackground => Label::MonitorSeriesBackground,
                GaugeKind::StateUnknown | GaugeKind::Hidden => Label::MonitorSeriesUnknown,
                GaugeKind::Sessions => Label::MonitorMetricSessions,
                GaugeKind::CommandQuery => Label::MonitorSeriesQuery,
                GaugeKind::CommandSleep => Label::MonitorSeriesSleep,
                GaugeKind::CommandDaemon => Label::MonitorSeriesDaemon,
                GaugeKind::CommandConnect => Label::MonitorSeriesConnect,
                GaugeKind::CommandBinlogDump => Label::MonitorSeriesBinlogDump,
                GaugeKind::CommandOther => Label::MonitorSeriesOther,
            },
        })
    }

    /// A metric's latest value as shown: `–` when there is none yet (or a gap).
    fn metric_value(&self, k: Metric) -> String {
        let last = self.m.history_of(k).and_then(|h| h.last());
        match (k, last) {
            (Metric::Sessions, Some(v)) => {
                let max = self.m.server.as_ref().and_then(|s| s.max_connections);
                match max {
                    Some(max) => format!("{}/{max}", v as u64),
                    None => format!("{}", v as u64),
                }
            }
            (Metric::CacheHit, Some(v)) => format!("{v:.1}%"),
            (Metric::Gauge(_), Some(v)) => format!("{}", v as u64),
            (Metric::Rate(CounterKind::BytesIn | CounterKind::BytesOut | CounterKind::TempBytes), Some(v)) => {
                format!("{}/s", crate::text::human_bytes(v.max(0.0) as u64))
            }
            (Metric::Rate(_), Some(v)) => rate(v),
            (_, None) => "–".into(),
        }
    }

    /// The sessions' states in a line (`active 6 · idle in transaction 2 · waiting 2`), from
    /// the gauges the format has.
    fn session_counts(&self) -> String {
        let Some(s) = &self.m.snapshot else { return String::new() };
        let g = |k: GaugeKind| s.gauges.iter().find(|g| g.kind == k).map(|g| g.value);
        let mut parts = Vec::new();
        for (k, l) in [
            (GaugeKind::Active, Label::MonitorCountActive),
            (GaugeKind::IdleInTx, Label::MonitorCountIdleInTx),
            (GaugeKind::WaitingForLock, Label::MonitorCountWaiting),
            (GaugeKind::Hidden, Label::MonitorCountHidden),
        ] {
            if let Some(n) = g(k).filter(|n| *n > 0 || k == GaugeKind::Active) {
                parts.push(format!("{} {n}", self.label(l)));
            }
        }
        parts.join(" · ")
    }

    fn values(&self, k: Metric) -> Vec<Option<f64>> {
        self.m.history_of(k).map(|h| h.points().map(|(_, v)| v).collect()).unwrap_or_default()
    }

    /// The Overview: chart tiles (one line each when narrow or short), the charts of several
    /// series below them (lines of their own in the list when narrow or short), then the
    /// longest running sessions.
    fn overview(&self, area: Rect, buf: &mut Buffer, hits: &mut MonitorHits) {
        let th = theme::cur();
        let metrics = self.metrics();
        if area.height == 0 || metrics.is_empty() {
            return;
        }
        let charts: Vec<ChartData> = self.charts();
        let compact = area.width < TILE_MIN * 2 || area.height < 24;
        let mut y = area.y;
        let bottom = area.y + area.height;
        if compact {
            // One line each: name, value, chart; five at most, the sessions get the rest.
            let n = metrics.len().min(5).min(area.height.saturating_sub(3) as usize);
            let shown = &metrics[..n];
            let x = area.x + 1;
            let w = area.width.saturating_sub(2);
            // One column of names and one of values for every line, metric or chart: the
            // values as wide as the widest shown, the names as wide as the longest but never
            // so wide that a chart's legend gets under [`COMPACT_LEGEND_MIN`] (longer names
            // are cut).
            let values: Vec<String> = shown.iter().map(|k| self.metric_value(*k)).collect();
            let value_w = values.iter().chain(charts.iter().map(|c| &c.total)).map(|v| width(v)).max().unwrap_or(0);
            let value_w = (value_w + 2).clamp(6, 11);
            let longest = shown
                .iter()
                .map(|k| width(&self.metric_label(*k)))
                .chain(charts.iter().map(|c| width(&c.title)))
                .max()
                .unwrap_or(0);
            let cap = usize::from(w).saturating_sub(value_w + COMPACT_LEGEND_MIN).max(6);
            let name_w = (longest + 2).min(cap);
            // The charts' lines go after the first rate and after the sessions (else last); a
            // legend wraps to a second line only while the longest running keep their room.
            let first_rate = shown.iter().position(|k| matches!(k, Metric::Rate(_)));
            let sessions = shown.iter().position(|k| *k == Metric::Sessions);
            let at = [first_rate.unwrap_or(n.saturating_sub(1)), sessions.unwrap_or(n.saturating_sub(1))];
            let mut spare = (area.height as usize).saturating_sub(n + charts.len() + 6);
            for (i, k) in shown.iter().enumerate() {
                put(buf, x, y, &self.metric_label(*k), name_w - 1, Style::new().fg(th.fg_muted));
                let vx = x + name_w as u16;
                let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
                put(buf, vx, y, &values[i], value_w - 1, bold);
                let sx = vx + value_w as u16;
                if sx < x + w {
                    let r = Rect { x: sx, y, width: x + w - sx, height: 1 };
                    sparkline::draw(&self.values(*k), r, Style::new().fg(th.accent), buf);
                }
                y += 1;
                for chart in charts.iter().zip(at).filter(|(_, a)| *a == i).map(|(c, _)| c) {
                    if y + 3 > bottom {
                        break;
                    }
                    let rows = 1 + usize::from(spare > 0);
                    let line = Rect { x, y, width: w, height: rows as u16 };
                    let used = self.chart_lines(chart, line, (name_w, value_w), buf);
                    spare -= used - 1;
                    y += used as u16;
                }
            }
        } else {
            let cols = (area.width / TILE_MIN).clamp(1, 4) as usize;
            let (layout, charts_h) = self.chart_layout(&charts, area);
            let rows_room = ((area.height.saturating_sub(6 + charts_h)) / 3).max(1) as usize;
            let rows = metrics.len().div_ceil(cols).min(rows_room);
            let tile_w = area.width / cols as u16;
            for (i, k) in metrics.iter().take(rows * cols).enumerate() {
                let (r, c) = (i / cols, i % cols);
                let x = area.x + c as u16 * tile_w;
                let w = if c + 1 == cols { area.width - c as u16 * tile_w } else { tile_w };
                let tile = Rect { x, y: area.y + r as u16 * 3, width: w, height: 3 };
                self.tile(*k, tile, buf);
            }
            y = area.y + rows as u16 * 3;
            let full = |h: u16| Rect { x: area.x, y: 0, width: area.width, height: h };
            match layout {
                ChartLayout::None => {}
                ChartLayout::Side(left) => {
                    let right = area.width - left;
                    self.chart_box(&charts[0], Rect { x: area.x, y, width: left, height: charts_h }, buf);
                    let at = Rect { x: area.x + left, y, width: right, height: charts_h };
                    self.chart_box(&charts[1], at, buf);
                }
                ChartLayout::Stacked(h0) => {
                    self.chart_box(&charts[0], Rect { y, ..full(h0) }, buf);
                    if let Some(second) = charts.get(1) {
                        self.chart_box(second, Rect { y: y + h0, ..full(charts_h - h0) }, buf);
                    }
                }
                ChartLayout::FirstAndLine(h0) => {
                    self.chart_box(&charts[0], Rect { y, ..full(h0) }, buf);
                    let rows = charts_h - h0;
                    let at = Rect { x: area.x + 1, y: y + h0, width: area.width.saturating_sub(2), height: rows };
                    let columns = (width(&charts[1].title) + 2, width(&charts[1].total) + 2);
                    self.chart_lines(&charts[1], at, columns, buf);
                }
            }
            y += charts_h;
        }
        if y + 2 > bottom {
            return;
        }
        // The longest running.
        put(
            buf,
            area.x + 1,
            y,
            &self.label(Label::MonitorLongest),
            area.width as usize - 2,
            Style::new().fg(th.fg_muted).add_modifier(Modifier::BOLD),
        );
        y += 1;
        let Some(list) = self.m.snapshot.as_ref().and_then(|s| s.sessions.as_ref()) else { return };
        let room = (bottom - y).saturating_sub(1) as usize;
        let top: Vec<usize> = self.m.top.iter().copied().take(room).collect();
        if top.is_empty() {
            put(
                buf,
                area.x + 1,
                y,
                &self.label(Label::MonitorNoneRunning),
                area.width as usize - 2,
                Style::new().fg(th.fg_dim),
            );
            return;
        }
        let cols = self.columns(area.width.saturating_sub(2));
        self.header(&cols, Rect { x: area.x + 1, y, width: area.width - 2, height: 1 }, buf, None);
        y += 1;
        for (n, &i) in top.iter().enumerate() {
            let at = Rect { x: area.x + 1, y: y + n as u16, width: area.width - 2, height: 1 };
            self.row(&cols, &list.rows[i], list.time, at, false, buf);
        }
        hits.rows = Rect { x: area.x, y, width: area.width, height: top.len() as u16 };
        hits.rows_view = Some(MonitorView::Overview);
        hits.top_rows = top;
    }

    /// A chart tile: its name and latest value on the border, the chart (or the sessions'
    /// states) inside.
    fn tile(&self, k: Metric, at: Rect, buf: &mut Buffer) {
        work(1);
        let th = theme::cur();
        let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(th.border));
        let inner = block.inner(at);
        block.render(at, buf);
        let name = format!(" {} ", self.metric_label(k));
        let value = format!(" {} ", self.metric_value(k));
        let room = at.width.saturating_sub(2) as usize;
        let vw = width(&value).min(room);
        put(buf, at.x + 1, at.y, &name, room.saturating_sub(vw + 1), Style::new().fg(th.fg_muted));
        put(
            buf,
            at.x + at.width - 1 - vw as u16,
            at.y,
            &value,
            vw,
            Style::new().fg(th.fg).add_modifier(Modifier::BOLD),
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        match k {
            Metric::Sessions => {
                let text = self.session_counts();
                put(buf, inner.x, inner.y, &text, inner.width as usize, Style::new().fg(th.fg));
            }
            _ => sparkline::draw(&self.values(k), inner, Style::new().fg(th.accent), buf),
        }
    }

    /// The charts of several series this format draws, in order.
    fn charts(&self) -> Vec<ChartData> {
        let defs: &[&ChartDef] = match self.m.format() {
            Some(MonitorFormat::MySql(_)) => &[&STATEMENTS_BY_TYPE, &THREAD_COMMANDS],
            Some(MonitorFormat::Postgres(_)) => &[&ROWS_BY_OPERATION, &SESSION_STATES],
            None => &[],
        };
        defs.iter().map(|d| self.chart(d)).collect()
    }

    fn chart(&self, def: &ChartDef) -> ChartData {
        let mut title = self.label(def.title);
        let own_only = self.m.snapshot.as_ref().is_some_and(|s| s.hidden.iter().any(|h| h.part == Part::OtherSessions));
        if def.own_account && own_only {
            title = format!("{title}: {}", self.label(Label::MonitorChartOwnAccount));
        }
        let mut series = Vec::new();
        for (colour, m) in def.series.iter().enumerate() {
            let Some(h) = self.m.history_of(*m) else { continue };
            let values: Vec<Option<f64>> = h.points().map(|(_, v)| v).collect();
            if def.only_above_zero == Some(*m) && !values.iter().flatten().any(|v| *v > 0.0) {
                continue;
            }
            // The rows read are a tile of their own too, named in full there.
            let name = match m {
                Metric::Rate(CounterKind::RowsRead) => self.label(Label::MonitorSeriesRead),
                _ => self.metric_label(*m),
            };
            series.push(Drawn { name, last: h.last(), values, colour });
        }
        let there = def.needs.is_none_or(|m| self.m.history_of(m).is_some()) && !series.is_empty();
        let missing = (!there).then(|| {
            let mut why = self.label(Label::MonitorChartNotAvailable);
            let off = self
                .m
                .server
                .as_ref()
                .is_some_and(|s| s.extra.iter().any(|e| e.key == ExtraKey::TrackCounts && e.value == "off"));
            if def.needs.is_some() && off {
                why.push_str(": track_counts = off");
            }
            why
        });
        if !there {
            series.clear();
        }
        let lasts: Vec<f64> = series.iter().filter_map(|s| s.last).collect();
        let mut data = ChartData { title, total: String::new(), series, rates: def.rates, missing };
        data.total = data.value((!lasts.is_empty()).then(|| lasts.iter().sum()));
        data
    }

    /// Where the charts go below one row of tiles: side by side when each gets [`CHART_MIN`]
    /// columns, else one under the other, else the first with the second as its compact line,
    /// all only when one row of tiles and the longest running's heading and three rows fit
    /// too; and the rows they take.
    fn chart_layout(&self, charts: &[ChartData], area: Rect) -> (ChartLayout, u16) {
        let fits = |h: u16| 3 + h + 4 <= area.height;
        let (layout, h) = match charts {
            [] => (ChartLayout::None, 0),
            [first, second, ..] if area.width >= CHART_MIN * 2 => {
                let left = area.width / 2;
                (ChartLayout::Side(left), first.height(left).max(second.height(area.width - left)))
            }
            [first] => {
                let h0 = first.height(area.width);
                (ChartLayout::Stacked(h0), h0)
            }
            [first, second, ..] => {
                let (h0, h1) = (first.height(area.width), second.height(area.width));
                // The second's line wraps its legend once when there is room.
                match (fits(h0 + h1), fits(h0 + 2)) {
                    (true, _) => (ChartLayout::Stacked(h0), h0 + h1),
                    (false, true) => (ChartLayout::FirstAndLine(h0), h0 + 2),
                    (false, false) => (ChartLayout::FirstAndLine(h0), h0 + 1),
                }
            }
        };
        if fits(h) { (layout, h) } else { (ChartLayout::None, 0) }
    }

    /// A chart of several series: its title and total on the border, the lines on one scale
    /// from zero (its top on the left), the legend under them.
    fn chart_box(&self, c: &ChartData, at: Rect, buf: &mut Buffer) {
        work(1);
        let th = theme::cur();
        let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(th.border));
        let inner = block.inner(at);
        block.render(at, buf);
        let value = format!(" {} ", c.total);
        let room = at.width.saturating_sub(2) as usize;
        let vw = width(&value).min(room);
        // The title's space after it gives way before the title does.
        let mut name = format!(" {} ", c.title);
        if width(&name) > room.saturating_sub(vw + 1) {
            name.pop();
        }
        put(buf, at.x + 1, at.y, &name, room.saturating_sub(vw + 1), Style::new().fg(th.fg_muted));
        let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
        put(buf, at.x + at.width - 1 - vw as u16, at.y, &value, vw, bold);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if let Some(why) = &c.missing {
            put(buf, inner.x, inner.y, why, inner.width as usize, Style::new().fg(th.fg_dim));
            return;
        }
        let legend = legend_rows(&c.legend(), &[inner.width as usize; LEGEND_ROWS]);
        let plot_h = inner.height.saturating_sub(legend.len() as u16).min(PLOT_ROWS);
        let colours = coloured(&th);
        // Without colours the lines cannot be told apart: the total alone.
        let total: Vec<Option<f64>>;
        let lines: Vec<(&[Option<f64>], Color)> = if colours {
            let palette = th.chart();
            c.series.iter().map(|s| (s.values.as_slice(), palette[s.colour % palette.len()])).collect()
        } else {
            let n = c.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            total = (0..n)
                .map(|i| {
                    let at: Vec<f64> = c.series.iter().filter_map(|s| s.values.get(i).copied().flatten()).collect();
                    (!at.is_empty()).then(|| at.iter().sum())
                })
                .collect();
            vec![(total.as_slice(), th.fg)]
        };
        let plain: Vec<&[Option<f64>]> = lines.iter().map(|(v, _)| *v).collect();
        let top_label = |w: u16| c.value(Some(lines::top(&plain, w)));
        // The scale's top on the first row, its zero on the last.
        let gutter = width(&top_label(inner.width)).max(1) as u16;
        let plot =
            Rect { x: inner.x + gutter + 1, y: inner.y, width: inner.width.saturating_sub(gutter + 1), height: plot_h };
        let any = plain.iter().any(|v| v.iter().any(Option::is_some));
        if plot.width > 0 && plot.height > 0 && any {
            let top = lines::top(&plain, plot.width);
            let muted = Style::new().fg(th.fg_dim);
            put(buf, inner.x, plot.y, &c.value(Some(top)), gutter as usize, muted);
            if plot.height > 1 {
                put(buf, inner.x, plot.y + plot.height - 1, "0", gutter as usize, muted);
            }
            lines::draw(&lines, top, plot, buf);
        }
        for (r, row) in legend.iter().enumerate() {
            legend_row(c, row, Rect { y: inner.y + plot_h + r as u16, height: 1, ..inner }, buf);
        }
    }

    /// A chart as lines of the metric list, in its columns (`(names, values)` wide): its
    /// title (cut with … when longer), its total, its legend where the others' charts start;
    /// the legend goes on in a second line at that column when `at` has two rows and needs it.
    /// Returns the rows used.
    fn chart_lines(&self, c: &ChartData, at: Rect, (name_w, value_w): (usize, usize), buf: &mut Buffer) -> usize {
        let th = theme::cur();
        let end = at.x + at.width;
        let name_w = name_w.min(at.width as usize);
        put(buf, at.x, at.y, &c.title, name_w.saturating_sub(1), Style::new().fg(th.fg_muted));
        let vx = at.x + name_w as u16;
        if vx >= end {
            return 1;
        }
        let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
        put(buf, vx, at.y, &c.total, value_w.saturating_sub(1).min(usize::from(end - vx)), bold);
        let lx = vx + value_w as u16;
        if lx >= end {
            return 1;
        }
        if let Some(why) = &c.missing {
            put(buf, lx, at.y, why, usize::from(end - lx), Style::new().fg(th.fg_dim));
            return 1;
        }
        let rows = legend_rows(&c.legend(), &vec![usize::from(end - lx); usize::from(at.height.max(1))]);
        for (r, row) in rows.iter().enumerate() {
            legend_row(c, row, Rect { x: lx, y: at.y + r as u16, width: end - lx, height: 1 }, buf);
        }
        rows.len().max(1)
    }

    /// The Sessions view: the header, the rows, the filter line, the details.
    fn sessions(&self, area: Rect, buf: &mut Buffer, hits: &mut MonitorHits) {
        let th = theme::cur();
        let m = self.m;
        let Some(list) = m.snapshot.as_ref().and_then(|s| s.sessions.as_ref()) else { return };
        if area.height < 2 {
            return;
        }
        let mut bottom = area.y + area.height;
        // The filter line.
        if m.list.filtering || !m.list.filter.text().is_empty() {
            bottom -= 1;
            let text = format!("/{}", m.list.filter.text());
            let style = if m.list.filtering { Style::new().fg(th.fg) } else { Style::new().fg(th.fg_muted) };
            put(buf, area.x + 1, bottom, &text, area.width as usize - 2, style);
        }
        // How many sessions, how many hidden for being idle.
        let shown = m.list.order.len();
        let mut count =
            self.msg(Msg::MonitorSessionCount { shown: shown.to_string(), total: list.rows.len().to_string() });
        if list.idle_left_out {
            let idle =
                m.snapshot.as_ref().and_then(|s| s.gauges.iter().find(|g| g.kind == GaugeKind::Idle)).map(|g| g.value);
            count.push_str(" · ");
            count.push_str(&match idle {
                Some(n) => self.msg(Msg::MonitorIdleHiddenCount { n: n.to_string() }),
                None => self.label(Label::MonitorIdleHiddenSome),
            });
        }
        put(buf, area.x + 1, area.y, &count, area.width as usize - 2, Style::new().fg(th.fg_muted));
        let mut y = area.y + 1;
        // The action list, at the bottom when there is room for a few rows above it.
        if m.actions_shown {
            let h = self.actions.len().clamp(1, ACTIONS_SHOWN) as u16 + 1;
            if bottom.saturating_sub(y) >= h + 3 {
                bottom -= h;
                self.action_list(Rect { x: area.x, y: bottom, width: area.width, height: h }, buf);
            }
        }
        // The details: below the list when there is room (the list at most half, no more than
        // its rows need: a laid-out statement takes lines), else in its place.
        let details = m.list.details.then(|| m.selected_row()).flatten();
        let (list_h, details_at) = match details {
            Some(_) if bottom - y >= 20 => {
                let h = ((bottom - y) / 2).min(shown.max(1) as u16 + 2);
                (h, Some(Rect { x: area.x, y: y + h, width: area.width, height: bottom - y - h }))
            }
            Some(_) => (0, Some(Rect { x: area.x, y, width: area.width, height: bottom - y })),
            None => (bottom - y, None),
        };
        if list_h >= 2 {
            let cols = self.columns(area.width.saturating_sub(2));
            self.header(&cols, Rect { x: area.x + 1, y, width: area.width - 2, height: 1 }, buf, Some(hits));
            y += 1;
            let page = (list_h - 1) as usize;
            // The selection on screen: the frame's own top when the cursor left it.
            let mut top = m.list.top.min(shown.saturating_sub(1));
            if m.list.cursor < top {
                top = m.list.cursor;
            } else if m.list.cursor >= top + page {
                top = m.list.cursor + 1 - page;
            }
            for (n, &i) in m.list.order.iter().skip(top).take(page).enumerate() {
                let at = Rect { x: area.x + 1, y: y + n as u16, width: area.width - 2, height: 1 };
                self.row(&cols, &list.rows[i], list.time, at, top + n == m.list.cursor, buf);
            }
            if shown == 0 {
                put(
                    buf,
                    area.x + 1,
                    y,
                    &self.label(Label::MonitorNoSessions),
                    area.width as usize - 2,
                    Style::new().fg(th.fg_dim),
                );
            }
            hits.rows = Rect { x: area.x, y, width: area.width, height: page as u16 };
            hits.rows_view = Some(MonitorView::Sessions);
        }
        if let (Some(r), Some(at)) = (details, details_at) {
            self.details(r, list.time, at, buf);
        }
    }

    /// The Locks view: how many wait, what is said of the reads, then the blocking tree (root
    /// blockers first, each session or transaction once, under the one it waits for).
    fn locks(&self, area: Rect, buf: &mut Buffer, hits: &mut MonitorHits) {
        let th = theme::cur();
        let m = self.m;
        let Some(list) = m.snapshot.as_ref().and_then(|s| s.sessions.as_ref()) else { return };
        if area.height < 2 {
            return;
        }
        let w = area.width.saturating_sub(2) as usize;
        let bottom = area.y + area.height;
        let mut y = area.y;
        let tree = &m.tree;
        // Sessions the account may not see (or not see wait) may wait all the same.
        let unseen = m.snapshot.as_ref().is_some_and(|s| {
            s.hidden.iter().any(|h| {
                matches!(
                    h.part,
                    Part::OtherSessionDetails | Part::OtherSessions | Part::UntrackedSessions | Part::TransactionState
                )
            })
        });
        let mut head = match tree.waiting {
            0 if unseen => self.label(Label::MonitorLocksNoneSeen),
            0 => self.label(Label::MonitorLocksNone),
            n => self.msg(Msg::MonitorLocksWaiting { n: n.to_string() }),
        };
        head.push_str(" · ");
        head.push_str(&match &m.locks {
            Some(l) => self.msg(Msg::MonitorLocksReadAgo { time: age(list.time.since(l.time).unwrap_or(0)) }),
            None => {
                let every = crate::app::monitor::LOCKS_EVERY.max(m.interval.duration());
                self.msg(Msg::MonitorLocksNotRead { every: format!("{}s", every.as_secs()) })
            }
        });
        put(buf, area.x + 1, y, &head, w, Style::new().fg(th.fg_muted));
        y += 1;
        // What the reads left out, and how far a holder is known to block.
        let mut notes: Vec<(String, Color)> = Vec::new();
        let waiting = m.snapshot.as_ref().and_then(|s| s.gauges.iter().find(|g| g.kind == GaugeKind::WaitingForLock));
        let asked = list.rows.iter().filter(|r| r.blocked_by.is_some()).count() as u64;
        if let Some(n) =
            waiting.map(|g| g.value.saturating_sub(asked)).filter(|n| *n > 0 && asked as usize >= MAX_WAITERS)
        {
            let max = MAX_WAITERS.to_string();
            notes.push((self.msg(Msg::MonitorLocksNotAsked { n: n.to_string(), max }), th.warning));
        }
        if m.locks.as_ref().is_some_and(|l| l.more > 0) || tree.partial {
            notes.push((self.label(Label::MonitorLocksMore), th.warning));
        }
        let unsure = |b: Blame| matches!(b, Blame::Incompatible | Blame::CannotTell);
        if tree.lines.iter().any(|l| l.parent.is_some_and(|(_, b)| unsure(b)) || l.also.iter().any(|(_, b)| unsure(*b)))
        {
            notes.push((self.label(Label::MonitorLocksIncompatibleNote), th.fg_dim));
        }
        for (text, color) in notes {
            for l in crate::text::wrap_words(&text, w).iter().take(2) {
                if y + 1 >= bottom {
                    break;
                }
                put(buf, area.x + 1, y, l, w, Style::new().fg(color));
                y += 1;
            }
        }
        let mut page = bottom.saturating_sub(y) as usize;
        if page == 0 {
            return;
        }
        // The selected line in full below the tree when it does not fit on its line.
        if let Some(i) = Some(m.lock_view.cursor).filter(|i| *i < tree.lines.len() && page >= 8) {
            let (text, color) = self.lock_line(i, list.time);
            let room = w.saturating_sub(2);
            if width(&text) > w {
                let lines = crate::text::wrap_words(&text, room);
                let n = lines.len().min(3);
                page -= n + 1;
                let at = Rect { x: area.x, y: y + page as u16, width: area.width, height: n as u16 + 1 };
                Block::default().borders(Borders::TOP).border_style(Style::new().fg(th.border)).render(at, buf);
                for (k, l) in lines.iter().take(n).enumerate() {
                    put(buf, area.x + 2, at.y + 1 + k as u16, l, room, Style::new().fg(color));
                }
            }
        }
        let top = on_screen(m.lock_view.cursor, m.lock_view.top, page);
        for (n, i) in (top..tree.lines.len()).take(page).enumerate() {
            work(1);
            let at = Rect { x: area.x + 1, y: y + n as u16, width: area.width.saturating_sub(2), height: 1 };
            let (text, color) = self.lock_line(i, list.time);
            let mut style = Style::new().fg(color);
            if i == m.lock_view.cursor {
                let sel = if self.focused { th.selection } else { th.selection_unfocused() };
                buf.set_style(at, sel);
                style = sel;
            }
            put(buf, at.x, at.y, &text, at.width as usize, style);
        }
        hits.rows = Rect { x: area.x, y, width: area.width, height: page as u16 };
        hits.rows_view = Some(MonitorView::Locks);
    }

    /// Line `i` of the blocking tree, and its colour: a root others wait behind in the error
    /// colour, a waiter in the warning one.
    fn lock_line(&self, i: usize, time: Timestamp) -> (String, Color) {
        let th = theme::cur();
        let m = self.m;
        let l = &m.tree.lines[i];
        let marks = m.tree_marks.get(i).map(String::as_str).unwrap_or("");
        let lead = if l.parent.is_none() { "▸ " } else { "  " };
        let mut parts: Vec<String> = Vec::new();
        let waits = l.parent.is_some() || l.alone || l.cycle;
        let mut color = if waits {
            th.warning
        } else if l.behind > 0 {
            th.error
        } else {
            th.fg
        };
        let head = match &l.node {
            Holder::Session(id) => {
                let r = m.session(*id);
                if r.is_some_and(|r| r.visible == Visibility::Hidden) {
                    color = th.fg_dim;
                }
                let user = r.and_then(|r| r.user.clone()).unwrap_or_default();
                if waits {
                    parts.push(self.waits_text(*id, r, time));
                    match l.parent.map(|(_, b)| b) {
                        Some(Blame::Incompatible) => parts.push(self.label(Label::MonitorLockIncompatible)),
                        Some(Blame::CannotTell) => parts.push(self.label(Label::MonitorLockCannotTell)),
                        _ => {}
                    }
                    if !l.also.is_empty() {
                        let who: Vec<String> = l.also.iter().map(|(h, _)| self.holder_name(h)).collect();
                        parts.push(self.msg(Msg::MonitorLockAlso { who: who.join(", ") }));
                    }
                    if l.alone && r.is_some_and(|r| r.blocked_by.is_none()) && !self.mysql {
                        // PostgreSQL: past the longest waits asked who blocks them.
                        let max = MAX_WAITERS.to_string();
                        parts.push(self.msg(Msg::MonitorLockNotAsked { max }));
                    } else if l.alone {
                        parts.push(self.label(Label::MonitorLockAlone));
                    }
                } else if let Some(r) = r {
                    let mut state = state_text(self.i18n, r);
                    if let Some(a) = tx_age(r, time) {
                        state.push(' ');
                        state.push_str(&age(a));
                    }
                    parts.push(state);
                }
                if let Some(holds) = self.holds(&l.node) {
                    parts.push(holds);
                }
                if l.cycle {
                    parts.push(self.label(Label::MonitorLockCycle));
                    color = th.error;
                }
                if let Some(q) = r.and_then(|r| r.query.as_ref()) {
                    parts.push(one_line(&q.text));
                }
                format!("{id} {user}")
            }
            Holder::Prepared(Some(p)) => {
                let time = p.since.and_then(|t| time.since(t)).map(age).unwrap_or_else(|| "?".into());
                parts.push(self.msg(Msg::MonitorLockPreparedDetail {
                    owner: p.owner.clone().unwrap_or_default(),
                    database: p.database.clone().unwrap_or_default(),
                    time,
                }));
                if let Some(holds) = self.holds(&l.node) {
                    parts.push(holds);
                }
                self.msg(Msg::MonitorLockPrepared { gid: p.gid.clone() })
            }
            Holder::Prepared(None) => self.label(Label::MonitorLockPreparedUnknown),
            Holder::Transaction { id, sessionless: true } => self.msg(Msg::MonitorLockTransaction { id: id.clone() }),
            Holder::Transaction { id, sessionless: false } => {
                self.msg(Msg::MonitorLockTransactionUnknown { id: id.clone() })
            }
        };
        let mut text = format!("{lead}{marks}{head}");
        for p in parts {
            text.push_str(" · ");
            text.push_str(&p);
        }
        (text, color)
    }

    /// A holder as a waiter's line names it.
    fn holder_name(&self, h: &Holder) -> String {
        match h {
            Holder::Session(id) => id.to_string(),
            Holder::Prepared(Some(p)) => self.msg(Msg::MonitorLockPrepared { gid: p.gid.clone() }),
            Holder::Prepared(None) => self.label(Label::MonitorPreparedTx),
            Holder::Transaction { id, .. } => id.clone(),
        }
    }

    /// What session `id` waits for, and for how long: from the lock read when it has the wait
    /// (of this very session: a reused id is another's), else from the session list.
    fn waits_text(&self, id: i64, r: Option<&SessionRow>, time: Timestamp) -> String {
        let wait = self.m.locks.as_ref().and_then(|l| l.waits_of(id).find(|w| r.is_none_or(|r| w.is_of(r))));
        match wait {
            Some(w) if !w.mode.is_empty() => {
                let object = object_text(&w.object);
                let mode = w.mode.clone();
                match w.since.or(r.and_then(|r| r.state_since)).and_then(|t| time.since(t)) {
                    Some(a) => self.msg(Msg::MonitorLockWaits { time: age(a), mode, object }),
                    None => self.msg(Msg::MonitorLockWaitsUntimed { mode, object }),
                }
            }
            _ => {
                let name = r.and_then(|r| r.wait.as_ref()).map(|w| w.name.clone()).unwrap_or_else(|| "?".into());
                self.msg(Msg::MonitorLockWaitsPlain { name })
            }
        }
    }

    /// What `node` holds that a session waits for, from the lock read.
    fn holds(&self, node: &Holder) -> Option<String> {
        let &(w, h) = self.m.held.get(node)?;
        let wait = self.m.locks.as_ref()?.waits.get(w)?;
        let h = wait.holders.get(h).filter(|h| !h.modes.is_empty())?;
        Some(self.msg(Msg::MonitorLockHolds { modes: h.modes.join(", "), object: object_text(&wait.object) }))
    }

    /// The actions sent from the tab, newest first: when, what, on which session, the answer
    /// and what the reads after it showed.
    fn action_list(&self, at: Rect, buf: &mut Buffer) {
        let th = theme::cur();
        let block = Block::default().borders(Borders::TOP).border_style(Style::new().fg(th.border));
        let inner = block.inner(at);
        block.render(at, buf);
        let title = format!(" {} ", self.label(Label::MonitorActions));
        put(buf, at.x + 1, at.y, &title, at.width as usize - 2, Style::new().fg(th.fg_muted));
        let w = inner.width.saturating_sub(2) as usize;
        if self.actions.is_empty() {
            put(buf, inner.x + 1, inner.y, &self.label(Label::MonitorActionsNone), w, Style::new().fg(th.fg_dim));
        }
        for (i, line) in self.actions.iter().take(inner.height as usize).enumerate() {
            put(buf, inner.x + 1, inner.y + i as u16, line, w, Style::new().fg(th.fg));
        }
    }

    /// The columns that fit in `w`, with their widths; the statement takes what is left.
    fn columns(&self, w: u16) -> Vec<(Col, u16)> {
        // Narrow: the id, user, state and age a little cut; wide: the state's words whole
        // ("idle in transaction").
        let narrow = w < 70;
        let (id, user, state, age) = match (narrow, w >= 90) {
            (true, _) => (6, 8, 11, 9),
            (false, false) => (7, 10, 12, 11),
            (false, true) => (7, 10, 19, 11),
        };
        let all = [
            (Col::Id, id),
            (Col::User, user),
            (Col::State, state),
            (Col::QueryAge, age),
            (Col::Wait, 14),
            (Col::Database, 10),
            (Col::TxAge, 11),
        ];
        // The statement keeps at least this much.
        let mut left = w.saturating_sub(if narrow { 12 } else { 14 });
        let mut cols = Vec::new();
        for (c, cw) in all {
            if cw < left {
                cols.push((c, cw));
                left -= cw + 1;
            }
        }
        cols.sort_by_key(|(c, _)| *c as u8);
        let used: u16 = cols.iter().map(|(_, cw)| cw + 1).sum();
        cols.push((Col::Query, w.saturating_sub(used)));
        cols
    }

    fn col_label(&self, c: Col) -> String {
        match c {
            Col::Id if self.mysql => "id".into(),
            Col::Id => "pid".into(),
            Col::User => self.label(Label::MonitorColUser),
            Col::Database => self.label(Label::MonitorColDatabase),
            Col::State => self.label(Label::MonitorColState),
            Col::Wait => self.label(Label::MonitorColWait),
            Col::TxAge => self.label(Label::MonitorColTxAge),
            Col::QueryAge => self.label(Label::MonitorColQueryAge),
            Col::Query => self.label(Label::MonitorColQuery),
        }
    }

    /// The column names; with `hits`, each takes a click that sorts by it, and the sorted one
    /// has its arrow.
    fn header(&self, cols: &[(Col, u16)], at: Rect, buf: &mut Buffer, hits: Option<&mut MonitorHits>) {
        let th = theme::cur();
        let mut x = at.x;
        let mut marks = Vec::new();
        for &(c, w) in cols {
            let mut text = self.col_label(c);
            if hits.is_some()
                && let Some(k) = c.sort()
            {
                if k == self.m.list.sort {
                    let down = matches!(k, SortKey::TxAge | SortKey::QueryAge) != self.m.list.reversed;
                    text.push_str(if down { " ▼" } else { " ▲" });
                }
                marks.push((Rect { x, y: at.y, width: w, height: 1 }, k));
            }
            put(buf, x, at.y, &text, w as usize, Style::new().fg(th.fg_muted).add_modifier(Modifier::BOLD));
            x += w + 1;
        }
        if let Some(h) = hits {
            h.headers = marks;
        }
    }

    /// One session's row.
    fn row(&self, cols: &[(Col, u16)], r: &SessionRow, time: Timestamp, at: Rect, selected: bool, buf: &mut Buffer) {
        work(1);
        let th = theme::cur();
        let long_tx = matches!(r.state, SessionState::IdleInTx | SessionState::IdleInTxAborted)
            && tx_age(r, time).is_some_and(|a| a > LONG_TX_US);
        let fg = if r.monitor || r.visible == Visibility::Hidden {
            th.fg_dim
        } else if r.waits_for_lock() || long_tx {
            th.warning
        } else {
            th.fg
        };
        let mut style = Style::new().fg(fg);
        if selected {
            let sel = if self.focused { th.selection } else { th.selection_unfocused() };
            buf.set_style(at, sel);
            style = sel;
        }
        // An action sent to it that still waits shows in its state, marked.
        let acting = self.m.action_on(&r.key).is_some();
        let mut x = at.x;
        for &(c, w) in cols {
            let text = self.cell(c, r, time);
            let style = if acting && c == Col::State && !selected { style.fg(th.warning) } else { style };
            put(buf, x, at.y, &text, w as usize, style);
            x += w + 1;
        }
    }

    fn cell(&self, c: Col, r: &SessionRow, time: Timestamp) -> String {
        let text = |s: &Option<String>| s.clone().unwrap_or_default();
        match c {
            Col::Id => r.key.id.to_string(),
            Col::User => text(&r.user),
            Col::Database => text(&r.database),
            Col::State if r.monitor => self.label(Label::MonitorOwnConnection),
            Col::State => match self.m.action_on(&r.key).map(|e| e.kind) {
                Some(datarig_core::monitor::ActionKind::Cancel) => self.label(Label::MonitorActStateCancelSent),
                Some(datarig_core::monitor::ActionKind::Terminate) => self.label(Label::MonitorActStateEndSent),
                None => self.state(r),
            },
            Col::Wait => r.wait.as_ref().map(|w| format!("{}:{}", wait_class(&w.class), w.name)).unwrap_or_default(),
            Col::TxAge => tx_age(r, time).map(age).unwrap_or_default(),
            Col::QueryAge => query_age(r, time).map(age).unwrap_or_default(),
            Col::Query => r.query.as_ref().map(|q| one_line(&q.text)).unwrap_or_default(),
        }
    }

    /// A session's state as the list shows it.
    fn state(&self, r: &SessionRow) -> String {
        state_text(self.i18n, r)
    }

    /// The selected session in full: who, where, since when, and its statement: its whole text
    /// once read for this very row, else as the list holds it (with why only its start is
    /// shown).
    fn details(&self, r: &SessionRow, time: Timestamp, at: Rect, buf: &mut Buffer) {
        let th = theme::cur();
        if at.height == 0 {
            return;
        }
        let block = Block::default().borders(Borders::TOP).border_style(Style::new().fg(th.border));
        let inner = block.inner(at);
        block.render(at, buf);
        put(
            buf,
            at.x + 1,
            at.y,
            &format!(" {} ", self.label(Label::MonitorDetails)),
            at.width as usize - 2,
            Style::new().fg(th.fg_muted),
        );
        let mut lines: Vec<(String, String)> = Vec::new();
        let mut field = |l: Label, v: Option<String>| {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                lines.push((self.label(l), v));
            }
        };
        let id = match r.monitor {
            true => format!("{} ({})", r.key.id, self.label(Label::MonitorOwnConnection)),
            false => r.key.id.to_string(),
        };
        field(Label::MonitorFieldSession, Some(id));
        field(Label::MonitorFieldTab, self.own.clone());
        field(Label::MonitorFieldKind, Some(self.kind(&r.kind)));
        field(Label::MonitorColUser, r.user.clone());
        field(Label::MonitorColDatabase, r.database.clone());
        field(Label::MonitorFieldApplication, r.application.clone());
        field(Label::MonitorFieldClient, r.client.clone());
        field(Label::MonitorColState, Some(self.state(r)));
        field(Label::MonitorColWait, r.wait.as_ref().map(|w| format!("{}:{}", wait_class(&w.class), w.name)));
        let blocked = r.blocked_by.as_ref().map(|b| {
            b.iter()
                .map(|b| match b {
                    datarig_core::monitor::Blocker::Session(id) => id.to_string(),
                    datarig_core::monitor::Blocker::PreparedTransaction => self.label(Label::MonitorPreparedTx),
                })
                .collect::<Vec<_>>()
                .join(", ")
        });
        field(Label::MonitorFieldBlockedBy, blocked);
        let tx = match r.tx {
            Tx::Since(t) => time.since(t).map(|a| self.msg(Msg::MonitorOpenFor { time: age(a) })),
            Tx::Open => Some(self.label(Label::MonitorTxOpenUnknown)),
            Tx::Unknown => Some(self.label(Label::MonitorTxUnknown)),
            Tx::None => Some(self.label(Label::MonitorTxNone)),
        };
        field(Label::MonitorFieldTx, tx);
        field(Label::MonitorFieldQueryStarted, query_age(r, time).map(|a| self.msg(Msg::MonitorAgo { time: age(a) })));
        field(
            Label::MonitorFieldStateSince,
            r.state_since.and_then(|t| time.since(t)).map(|a| self.msg(Msg::MonitorFor { time: age(a) })),
        );
        for e in &r.extra {
            lines.push((e.key.name().to_string(), e.value.clone()));
        }
        let key_w = lines.iter().map(|(k, _)| width(k)).max().unwrap_or(0) + 2;
        let text_w = inner.width.saturating_sub(2) as usize;
        // Each line of the details as its spans.
        let mut out: Vec<Vec<(String, Style)>> = Vec::new();
        let note = |out: &mut Vec<Vec<(String, Style)>>, text: &str, style: Style| {
            for l in crate::text::wrap_words(text, text_w) {
                out.push(vec![(l, style)]);
            }
        };
        for (k, v) in &lines {
            let text = format!("{k:<key_w$}{v}", key_w = key_w);
            out.push(vec![(text, Style::new().fg(th.fg))]);
        }
        if r.monitor {
            out.push(Vec::new());
            note(&mut out, &self.label(Label::MonitorOwnSession), Style::new().fg(th.fg_dim));
        }
        let read = self.m.text_read();
        let whole = match read.and_then(|t| t.answer.as_ref()) {
            Some(Ok(FullText::Text { text, proof })) => Some((text, *proof)),
            _ => None,
        };
        if let Some(q) = whole.map(|(text, _)| text).or(r.query.as_ref()) {
            let dim = Style::new().fg(th.fg_dim);
            out.push(Vec::new());
            if q.shortened {
                let text = self.msg(Msg::MonitorQueryShortened { n: datarig_core::monitor::QUERY_CHARS.to_string() });
                note(&mut out, &text, dim);
            }
            // Why the text shown is what it is, wrapped (a narrow screen keeps all of it).
            let why = match (whole, read.map(|t| &t.answer)) {
                (Some((_, Proof::Likely)), _) => Some((self.label(Label::MonitorTextLikely), dim)),
                (Some((_, Proof::Same)), _) | (None, None) => None,
                (None, Some(None)) => Some((self.label(Label::MonitorTextReading), dim)),
                (None, Some(Some(Ok(FullText::NotYet)))) => Some((self.label(Label::MonitorTextNotYet), dim)),
                (None, Some(Some(answer))) => {
                    let why = match answer {
                        Ok(FullText::Gone) => self.label(Label::MonitorTextGone),
                        Ok(FullText::Moved) => self.label(Label::MonitorTextMoved),
                        Ok(FullText::Unknown(Some(need))) => {
                            format!("{} ({})", self.label(Label::MonitorTextUnknown), self.need(need))
                        }
                        Ok(FullText::Unknown(None) | FullText::Text { .. } | FullText::NotYet) => {
                            self.label(Label::MonitorTextUnknown)
                        }
                        Err(e) => self.msg(Msg::MonitorTextFailed { error: e.clone() }),
                    };
                    Some((why, Style::new().fg(th.warning)))
                }
            };
            if let Some((text, style)) = why {
                note(&mut out, &text, style);
            }
            if q.maybe_cut_by_server {
                note(&mut out, &self.label(Label::MonitorQueryCutByServer), Style::new().fg(th.warning));
            }
            // The statement as shown (laid out for reading when it can be), in the editor's
            // colours; else as the server sent it.
            match self.m.statement.as_ref().filter(|s| s.source() == q.text) {
                Some(shown) => {
                    let key = self.format_key.clone();
                    let layout = match shown.layout {
                        Layout::Formatted => self.msg(Msg::MonitorLayoutFormatted { key }),
                        Layout::AsSent => self.msg(Msg::MonitorLayoutAsSent { key }),
                        Layout::Part => self.label(Label::MonitorLayoutPart),
                        Layout::Refused => self.label(Label::MonitorLayoutRefused),
                        Layout::TooLong => self.label(Label::MonitorLayoutTooLong),
                    };
                    note(&mut out, &layout, dim);
                    out.extend(statement_lines(shown, text_w));
                }
                None => {
                    for l in q.text.lines() {
                        for w in wrap(l, text_w) {
                            out.push(vec![(w, Style::new().fg(th.fg))]);
                        }
                    }
                }
            }
        }
        let top = self.m.list.details_top.min(out.len().saturating_sub(1));
        for (i, spans) in out.iter().skip(top).take(inner.height as usize).enumerate() {
            let mut x = inner.x + 1;
            let mut room = text_w;
            for (text, style) in spans {
                let w = put(buf, x, inner.y + i as u16, text, room, *style);
                x += w;
                room = room.saturating_sub(w as usize);
            }
        }
    }

    fn kind(&self, k: &SessionKind) -> String {
        match k {
            SessionKind::Client => self.label(Label::MonitorKindClient),
            SessionKind::Background(name) => name.clone(),
            SessionKind::Replication => self.label(Label::MonitorKindReplication),
            SessionKind::ParallelWorker { leader: Some(l) } => {
                self.msg(Msg::MonitorKindWorkerOf { leader: l.to_string() })
            }
            SessionKind::ParallelWorker { leader: None } => self.label(Label::MonitorKindWorker),
            SessionKind::Unknown => self.label(Label::MonitorKindUnknown),
        }
    }
}

/// A column of the session list, in screen order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col {
    Id = 0,
    User = 1,
    Database = 2,
    State = 3,
    Wait = 4,
    TxAge = 5,
    QueryAge = 6,
    Query = 7,
}

impl Col {
    fn sort(self) -> Option<SortKey> {
        Some(match self {
            Col::Id => SortKey::Id,
            Col::User => SortKey::User,
            Col::Database => SortKey::Database,
            Col::State => SortKey::State,
            Col::TxAge => SortKey::TxAge,
            Col::QueryAge => SortKey::QueryAge,
            Col::Wait | Col::Query => return None,
        })
    }
}

/// A session's state in the UI's words: an idle session whose transaction the account cannot
/// see says so, a server process its name, another state the server's own word.
pub(crate) fn state_text(i18n: &I18n, r: &SessionRow) -> String {
    let l = |l: Label| i18n.label(l).to_string();
    match (&r.state, &r.kind) {
        (SessionState::Background, SessionKind::Background(name)) => name.clone(),
        (SessionState::Active, _) => l(Label::MonitorStateActive),
        (SessionState::Idle, _) if r.tx == Tx::Unknown => format!("{} ?", l(Label::MonitorStateIdle)),
        (SessionState::Idle, _) => l(Label::MonitorStateIdle),
        (SessionState::IdleInTx, _) => l(Label::MonitorStateIdleInTx),
        (SessionState::IdleInTxAborted, _) => l(Label::MonitorStateIdleInTxAborted),
        (SessionState::Background, _) => l(Label::MonitorStateBackground),
        (SessionState::Unknown, _) => "?".into(),
        (SessionState::Other(s), _) => s.clone(),
    }
}

/// A wait class by the server's word for it.
fn wait_class(c: &WaitClass) -> &str {
    match c {
        WaitClass::Lock => "Lock",
        WaitClass::LightweightLock => "LWLock",
        WaitClass::Io => "IO",
        WaitClass::Client => "Client",
        WaitClass::Ipc => "IPC",
        WaitClass::Timeout => "Timeout",
        WaitClass::Activity => "Activity",
        WaitClass::Other(s) => s,
    }
}

/// The first line of a page of `page` lines that keeps line `cursor` on screen, from `top`.
fn on_screen(cursor: usize, top: usize, page: usize) -> usize {
    if cursor < top {
        cursor
    } else if cursor >= top + page {
        cursor + 1 - page
    } else {
        top
    }
}

/// What a lock is on, in the server's words: `relation zz_mon_t`, `transactionid 736`,
/// `RECORD shop.orders PRIMARY 42` (the database when another than the monitor's).
fn object_text(o: &LockObject) -> String {
    let mut parts: Vec<&str> = vec![o.kind.as_str()];
    parts.extend(o.name.as_deref());
    parts.extend(o.detail.as_deref());
    let mut text = parts.join(" ");
    if let Some(db) = &o.database {
        text.push_str(&format!(" ({db})"));
    }
    text
}

/// The statement `shown` in rows of `w` columns (broken where a row is full, as the server's
/// lines and the layout's), each token in its syntax colour.
fn statement_lines(shown: &Shown, w: usize) -> Vec<Vec<(String, Style)>> {
    use unicode_segmentation::UnicodeSegmentation;
    let th = theme::cur();
    let w = w.max(2);
    let plain = Style::new().fg(th.fg);
    let text = shown.text.as_str();
    let mut out = Vec::new();
    let mut tok = 0;
    let mut start = 0;
    for raw in text.split('\n') {
        let mut row: Vec<(String, Style)> = Vec::new();
        let mut used = 0;
        let line_end = start + raw.len();
        for (at, g) in raw.grapheme_indices(true) {
            let off = start + at;
            if g == "\r" && off + 1 == line_end {
                continue;
            }
            while tok < shown.tokens.len() && shown.tokens[tok].0.end <= off {
                tok += 1;
            }
            let style = match shown.tokens.get(tok) {
                Some((r, kind, call)) if r.start <= off => th.syntax(*kind, *call),
                _ => plain,
            };
            let (g, gw) = if g == "\t" { ("    ", 4) } else { (g, width(g)) };
            if used + gw > w && used > 0 {
                out.push(std::mem::take(&mut row));
                used = 0;
            }
            match row.last_mut() {
                Some((t, s)) if *s == style => t.push_str(g),
                _ => row.push((g.to_string(), style)),
            }
            used += gw;
        }
        out.push(row);
        start = line_end + 1;
    }
    out
}

/// A statement on one line: its whitespace runs as one space.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A duration in microseconds, short: `850ms`, `12s`, `3m12s`, `4h05m`, `41d3h`.
pub(crate) fn age(us: i64) -> String {
    let us = us.max(0);
    if us < 1_000_000 {
        return format!("{}ms", us / 1000);
    }
    let s = us / 1_000_000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        3600..86_400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
    }
}

/// A rate, short: `0.4`, `412`, `81.2 k`, `2.1 M`.
pub(crate) fn rate(v: f64) -> String {
    let v = v.max(0.0);
    if v < 10.0 {
        format!("{v:.1}")
    } else if v < 1000.0 {
        format!("{v:.0}")
    } else if v < 1_000_000.0 {
        format!("{:.1} k", v / 1000.0)
    } else {
        format!("{:.1} M", v / 1_000_000.0)
    }
}

/// The server's version line, short: PostgreSQL's up to its build details, MySQL's with its
/// product name.
fn short_version(line: &str, mysql: bool) -> String {
    if mysql {
        // `8.4.11 MySQL Community Server - GPL`: the product's name first.
        let (number, comment) = line.split_once(' ').unwrap_or((line, ""));
        let product = comment.split(" - ").next().unwrap_or("").trim();
        return match product.is_empty() {
            true => format!("MySQL {number}"),
            false => format!("{number} ({product})"),
        };
    }
    let end = [" (", " on "].iter().filter_map(|p| line.find(p)).min().unwrap_or(line.len());
    clip(&line[..end], 40)
}

/// The engine-only facts of a session, by the server's names (for tests).
#[allow(dead_code)]
fn extra_names(r: &SessionRow) -> Vec<&'static str> {
    r.extra.iter().map(|e| ExtraKey::name(e.key)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_and_rates_read_short() {
        assert_eq!(age(850_000), "850ms");
        assert_eq!(age(12_000_000), "12s");
        assert_eq!(age(192_000_000), "3m12s");
        assert_eq!(age(4 * 3600 * 1_000_000 + 5 * 60 * 1_000_000), "4h05m");
        assert_eq!(age((41 * 86_400 + 3 * 3600) * 1_000_000), "41d3h");
        assert_eq!(age(-5), "0ms");
        assert_eq!(rate(0.42), "0.4");
        assert_eq!(rate(412.4), "412");
        assert_eq!(rate(81_234.0), "81.2 k");
        assert_eq!(rate(2_100_000.0), "2.1 M");
    }

    #[test]
    fn versions_read_short() {
        let pg = "PostgreSQL 17.11 (Debian 17.11-1.pgdg13+2) on aarch64-unknown-linux-gnu, compiled by gcc";
        assert_eq!(short_version(pg, false), "PostgreSQL 17.11");
        assert_eq!(short_version("8.4.11 MySQL Community Server - GPL", true), "8.4.11 (MySQL Community Server)");
        assert_eq!(short_version("8.0.45", true), "MySQL 8.0.45");
    }
}
