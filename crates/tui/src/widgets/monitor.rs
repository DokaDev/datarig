//! The Monitor tab: the server's strip, the views' strip, what the account cannot see, then the
//! Overview (charts and the longest running sessions) or the Sessions (the list, its details).
//!
//! A frame draws from the tab's latest read: the list was sorted and filtered when it came
//! (or when the sort or the filter changed), so a frame only looks at the rows on screen,
//! however many sessions the server has.

use crate::app::action::{Action, MonitorAction};
use crate::app::monitor::{METRICS, Metric, MonState, MonitorHits, MonitorTab, MonitorView, SortKey};
use crate::app::monitor::{query_age, tx_age};
use crate::app::{App, Focus};
use crate::keymap::Ctx;
use crate::text::{clip, width, wrap};
use crate::theme;
use crate::widgets::{panel, put, sparkline};
use datarig_core::i18n::{I18n, Label, Msg};
use datarig_core::monitor::{
    CounterKind, ExtraKey, FullText, GaugeKind, Hidden, MonitorFormat, Need, Part, Proof, SessionKind, SessionRow,
    SessionState, Timestamp, Tx, Visibility, WaitClass,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

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

/// A session in a transaction for longer than this (idle in it) is marked.
const LONG_TX_US: i64 = 60_000_000;

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
    let Some(m) = app.monitor_of(id) else { return };
    let mysql = matches!(m.format(), Some(MonitorFormat::MySql(_)));
    let cx = Cx { i18n: &app.i18n, m, focused, mysql, read_key };
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
        },
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
        m.hits = hits;
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

    /// `1 Overview  2 Sessions`, then how the tab reads on the right.
    fn view_strip(&self, at: Rect, buf: &mut Buffer, hits: &mut MonitorHits) {
        let th = theme::cur();
        let mut x = at.x + 1;
        let end = at.x + at.width.saturating_sub(1);
        for (i, v) in MonitorView::ALL.iter().enumerate() {
            let text = format!("{} {}", i + 1, self.label(v.label()));
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
        let (state, color) = self.reading();
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
        }
    }

    fn hidden(&self, h: &Hidden) -> String {
        let need = self.need(&h.need);
        let part = match h.part {
            Part::OtherSessionDetails => Label::MonitorHiddenDetails,
            Part::OtherSessions => Label::MonitorHiddenSessions,
            Part::TransactionState => Label::MonitorHiddenTx,
            Part::UntrackedSessions => Label::MonitorHiddenUntracked,
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

    /// The Overview: chart tiles (one line each when narrow or short), then the longest
    /// running sessions.
    fn overview(&self, area: Rect, buf: &mut Buffer, hits: &mut MonitorHits) {
        let th = theme::cur();
        let metrics = self.metrics();
        if area.height == 0 || metrics.is_empty() {
            return;
        }
        let compact = area.width < TILE_MIN * 2 || area.height < 24;
        let mut y = area.y;
        let bottom = area.y + area.height;
        if compact {
            // One line each: name, value, chart; five at most, the sessions get the rest.
            let n = metrics.len().min(5).min(area.height.saturating_sub(3) as usize);
            let name_w = metrics.iter().take(n).map(|k| width(&self.metric_label(*k))).max().unwrap_or(0) + 2;
            for k in metrics.iter().take(n) {
                let x = area.x + 1;
                let w = area.width.saturating_sub(2);
                put(buf, x, y, &self.metric_label(*k), name_w, Style::new().fg(th.fg_muted));
                let value = self.metric_value(*k);
                let vx = x + name_w as u16;
                put(buf, vx, y, &value, 10, Style::new().fg(th.fg).add_modifier(Modifier::BOLD));
                let sx = vx + 11;
                if sx < x + w {
                    let r = Rect { x: sx, y, width: x + w - sx, height: 1 };
                    sparkline::draw(&self.values(*k), r, Style::new().fg(th.accent), buf);
                }
                y += 1;
            }
        } else {
            let cols = (area.width / TILE_MIN).clamp(1, 4) as usize;
            let rows_room = ((area.height.saturating_sub(6)) / 3).max(1) as usize;
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
        // The details: below the list when there is room, else in its place.
        let details = m.list.details.then(|| m.selected_row()).flatten();
        let (list_h, details_at) = match details {
            Some(_) if bottom - y >= 20 => {
                let h = (bottom - y) / 2;
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
        let mut x = at.x;
        for &(c, w) in cols {
            let text = self.cell(c, r, time);
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
            Col::State => self.state(r),
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
        field(Label::MonitorFieldSession, Some(r.key.id.to_string()));
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
        let mut out: Vec<(String, Style)> = Vec::new();
        for (k, v) in &lines {
            let text = format!("{k:<key_w$}{v}", key_w = key_w);
            out.push((text, Style::new().fg(th.fg)));
        }
        let read = self.m.text_read();
        let whole = match read.and_then(|t| t.answer.as_ref()) {
            Some(Ok(FullText::Text { text, proof })) => Some((text, *proof)),
            _ => None,
        };
        if let Some(q) = whole.map(|(text, _)| text).or(r.query.as_ref()) {
            let dim = Style::new().fg(th.fg_dim);
            out.push((String::new(), Style::new()));
            if q.shortened {
                out.push((
                    self.msg(Msg::MonitorQueryShortened { n: datarig_core::monitor::QUERY_CHARS.to_string() }),
                    dim,
                ));
            }
            // Why the text shown is what it is, wrapped (a narrow screen keeps all of it).
            let note = match (whole, read.map(|t| &t.answer)) {
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
            if let Some((text, style)) = note {
                for l in crate::text::wrap_words(&text, inner.width.saturating_sub(2) as usize) {
                    out.push((l, style));
                }
            }
            if q.maybe_cut_by_server {
                out.push((self.label(Label::MonitorQueryCutByServer), Style::new().fg(th.warning)));
            }
            for l in q.text.lines() {
                for w in wrap(l, inner.width.saturating_sub(2) as usize) {
                    out.push((w, Style::new().fg(th.syn_identifier.fg.unwrap_or(th.fg))));
                }
            }
        }
        let top = self.m.list.details_top.min(out.len().saturating_sub(1));
        for (i, (text, style)) in out.iter().skip(top).take(inner.height as usize).enumerate() {
            put(buf, inner.x + 1, inner.y + i as u16, text, inner.width.saturating_sub(2) as usize, *style);
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
