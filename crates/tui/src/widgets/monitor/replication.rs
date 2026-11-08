//! The Replication view (what the server takes changes from, the servers that take changes from
//! it, PostgreSQL's slots, MySQL's group) and the server's facts in full, both as lines that
//! scroll. A lag is worded as the core tells it ([`Lag`]): applied all it received, how much it
//! has not applied, stopped, or not known; never a figure that hides what it does not measure.

use super::{Cx, age, put};
use crate::app::monitor::{MonitorHits, MonitorView};
use crate::text::{human_bytes, width};
use crate::theme;
use datarig_core::i18n::{Label, Msg};
use datarig_core::monitor::replication::{ChannelError, Member, Replica, Running, Side, Slot, Upstream};
use datarig_core::monitor::{ExtraKey, Flavour, GaugeKind, Lag, Replication, Role, Timestamp, Visibility};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

/// Lines of text, each with its style.
type Lines = Vec<(String, Style)>;

/// How far the lines under a heading are set in.
const INDENT: &str = "  ";

/// The widest key of the server's facts that keeps its value on its own line.
const KEY_MAX: usize = 18;

impl Cx<'_> {
    /// The Replication view in `area`, from its line `top` (kept within the lines): the top drawn.
    pub(super) fn replication(&self, area: Rect, buf: &mut Buffer, hits: &mut MonitorHits) -> usize {
        let lines = self.replication_lines(area.width.saturating_sub(2) as usize);
        hits.rows = area;
        hits.rows_view = Some(MonitorView::Replication);
        draw_lines(&lines, self.m.replication_top, area, buf)
    }

    fn replication_lines(&self, w: usize) -> Lines {
        let th = theme::cur();
        let m = self.m;
        let mut out = Lines::new();
        let dim = Style::new().fg(th.fg_dim);
        let head = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
        if let Some(e) = &m.replication_error {
            wrapped(&mut out, "", &self.msg(Msg::MonitorReplFailed { error: e.clone() }), w, Style::new().fg(th.error));
        }
        let Some(r) = &m.replication else {
            if m.replication_error.is_none() {
                out.push((self.label(Label::MonitorReplReading), dim));
            }
            return out;
        };
        for h in &r.hidden {
            wrapped(&mut out, "", &self.hidden(h), w, Style::new().fg(th.warning));
        }
        if m.format().map(|f| f.flavour()) == Some(Flavour::AmazonAurora) {
            wrapped(&mut out, "", &self.label(Label::MonitorReplAurora), w, dim);
        }
        // Aurora's own replicas are not in these views: "not replicating" would not be known.
        let aurora = m.format().map(|f| f.flavour()) == Some(Flavour::AmazonAurora);
        let role = match r.role() {
            Role::Standalone if aurora => Label::MonitorReplRoleUnknown,
            Role::Replica if r.in_recovery == Some(true) => Label::MonitorReplRoleStandby,
            // Not in recovery: it takes changes by its subscriptions, and writes of its own.
            Role::Replica if r.in_recovery == Some(false) => Label::MonitorReplRoleSubscriber,
            Role::Replica => Label::MonitorReplRoleReplica,
            Role::Primary => Label::MonitorReplRolePrimary,
            Role::Standalone => Label::MonitorReplRoleStandalone,
            Role::Unknown => Label::MonitorReplRoleUnknown,
        };
        let mut first = vec![self.msg(Msg::MonitorReplRole { role: self.label(role) })];
        if let Some(p) = r.position {
            first.push(self.msg(Msg::MonitorReplPosition { lsn: p.to_string() }));
        }
        out.push((first.join(" · "), head));
        for u in r.upstream.iter().flatten() {
            out.push((String::new(), Style::new()));
            self.upstream(u, r.time, w, &mut out);
        }
        if let Some(list) = &r.replicas {
            out.push((String::new(), Style::new()));
            out.push((self.msg(Msg::MonitorReplReplicas { n: list.len().to_string() }), head));
            if list.is_empty() {
                out.push((format!("{INDENT}{}", self.label(Label::MonitorReplNoReplicas)), dim));
            }
            for rep in list {
                self.replica(rep, r, w, &mut out);
            }
        }
        if let Some(slots) = &r.slots {
            out.push((String::new(), Style::new()));
            out.push((self.msg(Msg::MonitorReplSlots { n: slots.len().to_string() }), head));
            if slots.is_empty() {
                out.push((format!("{INDENT}{}", self.label(Label::MonitorReplNoSlots)), dim));
            }
            for s in slots {
                let (parts, style) = self.slot(s);
                pack(&mut out, INDENT, parts, w, style);
            }
        }
        if let Some(subs) = r.subscriptions.as_ref().filter(|s| !s.is_empty()) {
            out.push((String::new(), Style::new()));
            out.push((self.msg(Msg::MonitorReplSubscriptions { n: subs.len().to_string() }), head));
            for sub in subs {
                let mut parts = vec![sub.name.clone()];
                parts.push(self.running(if sub.running { &Running::Yes } else { &Running::No }));
                if let Some(lsn) = sub.received {
                    parts.push(self.msg(Msg::MonitorReplReceived { lsn: lsn.to_string() }));
                }
                if let Some(a) = sub.last_message.and_then(|t| r.time.since(t)) {
                    parts.push(self.msg(Msg::MonitorReplLastMessage { time: age(a) }));
                }
                let style = if sub.running { Style::new().fg(th.fg) } else { Style::new().fg(th.warning) };
                pack(&mut out, INDENT, parts, w, style);
            }
        }
        if !r.members.is_empty() {
            out.push((String::new(), Style::new()));
            out.push((self.msg(Msg::MonitorReplMembers { n: r.members.len().to_string() }), head));
            for member in &r.members {
                let (parts, style) = self.member(member);
                pack(&mut out, INDENT, parts, w, style);
            }
        }
        out
    }

    /// What this server takes changes from: where from, whether each side runs, how far behind.
    fn upstream(&self, u: &Upstream, now: Timestamp, w: usize, out: &mut Lines) {
        let th = theme::cur();
        let text = Style::new().fg(th.fg);
        let dim = Style::new().fg(th.fg_dim);
        let ago = |t: Option<Timestamp>| t.and_then(|t| now.since(t)).map(age);
        // Where from.
        let source = u.source.clone().unwrap_or_else(|| "?".into());
        let mut head = match (&u.channel, self.mysql) {
            (Some(name), _) => self.msg(Msg::MonitorReplChannel { name: name.clone(), source }),
            (None, true) => self.msg(Msg::MonitorReplChannelDefault { source }),
            (None, false) if u.receiver == Running::No => self.label(Label::MonitorReplNoReceiver),
            (None, false) => self.msg(Msg::MonitorReplSource { source }),
        };
        if let Some(slot) = &u.slot {
            head.push_str(" · ");
            head.push_str(&self.msg(Msg::MonitorReplSlot { name: slot.clone() }));
        }
        out.push((head, Style::new().fg(th.fg).add_modifier(Modifier::BOLD)));
        if u.visible == Visibility::Hidden {
            out.push((format!("{INDENT}{}", self.label(Label::MonitorReplStreamHidden)), dim));
        }
        // The receiving side.
        let receiver_label =
            self.label(if self.mysql { Label::MonitorReplReceiver } else { Label::MonitorReplWalReceiver });
        let mut parts = vec![format!("{receiver_label}: {}", self.running(&u.receiver))];
        let mut style = run_style(&u.receiver);
        if let Some(state) = &u.receiver_state {
            parts.push(state.clone());
        }
        // A MySQL source sends a heartbeat only while it has nothing else to send: its age is
        // no measure of how long the source has been silent.
        if let Some(a) = ago(u.last_message) {
            parts.push(match self.mysql {
                true => self.msg(Msg::MonitorReplLastHeartbeat { time: a }),
                false => self.msg(Msg::MonitorReplLastMessage { time: a }),
            });
        }
        if let Some(a) = ago(u.last_received) {
            parts.push(self.msg(Msg::MonitorReplLastFetched { time: a }));
        }
        // A PostgreSQL standby without a receiver says so in its heading.
        if u.receiver != Running::No || self.mysql {
            pack(out, INDENT, parts, w, style);
        }
        // The applying side.
        let applier_label = self.label(if self.mysql { Label::MonitorReplApplier } else { Label::MonitorReplReplay });
        let mut parts = vec![format!("{applier_label}: {}", self.running(&u.applier))];
        style = run_style(&u.applier);
        if let Some(state) = &u.applier_state {
            parts.push(state.clone());
        }
        if self.mysql
            && let Some(a) = ago(u.last_applied)
        {
            parts.push(self.msg(Msg::MonitorReplLastApplied { time: a }));
        }
        pack(out, INDENT, parts, w, style);
        // How far behind.
        let (lag, lag_style) = self.lag(&u.lag(), true);
        wrapped(out, INDENT, &lag, w, lag_style);
        if self.mysql && u.applier_behind == Some(0) && u.receiver != Running::Yes {
            wrapped(out, INDENT, &self.label(Label::MonitorReplZeroNote), w, dim);
        }
        if let (Some(read), Some(applied)) = (&u.source_read, &u.source_applied) {
            let parts = vec![
                self.msg(Msg::MonitorReplLogRead { pos: read.to_string() }),
                self.msg(Msg::MonitorReplLogApplied { pos: applied.to_string() }),
            ];
            pack(out, INDENT, parts, w, text);
        }
        if let (Some(received), Some(applied)) = (u.received, u.applied) {
            let positions =
                self.msg(Msg::MonitorReplPositions { received: received.to_string(), replayed: applied.to_string() });
            out.push((format!("{INDENT}{positions}"), text));
        } else if let Some(applied) = u.applied {
            let replayed = self.msg(Msg::MonitorReplReplayed { replayed: applied.to_string() });
            out.push((format!("{INDENT}{replayed}"), text));
        }
        if !self.mysql
            && let Some(a) = ago(u.last_applied)
        {
            wrapped(out, INDENT, &self.msg(Msg::MonitorReplLastReplayed { time: a }), w, dim);
        }
        if let Some(d) = u.delay.filter(|d| *d > 0) {
            out.push((format!("{INDENT}{}", self.msg(Msg::MonitorReplDelay { seconds: d.to_string() })), text));
        }
        for e in &u.errors {
            wrapped(out, INDENT, &self.channel_error(e), w, Style::new().fg(th.error));
        }
        for (label, set) in
            [(Label::MonitorReplGtidsReceived, &u.received_set), (Label::MonitorReplGtidsExecuted, &u.executed_set)]
        {
            // A set may hold many intervals: only its start is shown, so a frame's work stays
            // bounded whatever its size.
            if let Some(set) = set.as_deref().map(set_start).filter(|s| !s.is_empty()) {
                wrapped(out, INDENT, &format!("{} {set}", self.label(label)), w, dim);
            }
        }
    }

    /// A lag in words, and its style; `received`: worded against what the server received (a
    /// replica's own view), else against what was written (a primary's view of its replica).
    fn lag(&self, lag: &Lag, received: bool) -> (String, Style) {
        let th = theme::cur();
        match lag {
            Lag::CaughtUp if received => (self.label(Label::MonitorLagCaughtUp), Style::new().fg(th.fg)),
            Lag::CaughtUp => (self.label(Label::MonitorLagReplayedAll), Style::new().fg(th.fg)),
            Lag::Behind { bytes, transactions, seconds } => {
                let mut parts = Vec::new();
                if let Some(b) = bytes {
                    let bytes = human_bytes(*b);
                    parts.push(match self.mysql {
                        true => self.msg(Msg::MonitorLagBytesFetched { bytes }),
                        false => self.msg(Msg::MonitorLagBytes { bytes }),
                    });
                }
                if let Some(n) = transactions {
                    parts.push(self.msg(Msg::MonitorLagTransactions { n: n.to_string() }));
                }
                if let Some(s) = seconds {
                    parts.push(self.msg(Msg::MonitorLagSeconds { n: s.to_string() }));
                }
                // Behind, by how much not known (positions in two files, no count, no seconds).
                if parts.is_empty() {
                    parts.push(self.label(Label::MonitorLagBehindUnknown));
                }
                (parts.join(" · "), Style::new().fg(th.warning))
            }
            Lag::Stopped => (self.label(Label::MonitorLagStopped), Style::new().fg(th.error)),
            Lag::Unknown => (self.label(Label::MonitorLagUnknown), Style::new().fg(th.fg_dim)),
        }
    }

    fn running(&self, r: &Running) -> String {
        match r {
            Running::Yes => self.label(Label::MonitorRunYes),
            Running::No => self.label(Label::MonitorRunNo),
            Running::Connecting => self.label(Label::MonitorRunConnecting),
            Running::Paused => self.label(Label::MonitorRunPaused),
            Running::Other(s) => s.clone(),
            Running::Unknown => "?".into(),
        }
    }

    fn channel_error(&self, e: &ChannelError) -> String {
        let side = self.label(match e.side {
            Side::Receiver => Label::MonitorReplReceiver,
            Side::Applier => Label::MonitorReplApplier,
        });
        let message = one_line(&e.message);
        match &e.at {
            Some(at) => self.msg(Msg::MonitorReplErrorAt { side, code: e.code.to_string(), at: at.clone(), message }),
            None => self.msg(Msg::MonitorReplError { side, code: e.code.to_string(), message }),
        }
    }

    /// A server that takes changes from this one: who, and (PostgreSQL) how far behind.
    fn replica(&self, rep: &Replica, r: &Replication, w: usize, out: &mut Lines) {
        let th = theme::cur();
        let dim = Style::new().fg(th.fg_dim);
        let mut who = Vec::new();
        if self.mysql {
            who.extend(rep.client.clone());
            who.extend(rep.user.clone());
            who.extend(rep.state.clone());
        } else {
            who.extend(rep.name.clone());
            who.extend(rep.client.clone());
            who.extend(rep.state.clone());
            who.extend(rep.sync.clone());
            if rep.visible == Visibility::Hidden {
                who.extend(rep.id.map(|pid| format!("pid {pid}")));
            }
        }
        pack(out, INDENT, who, w, Style::new().fg(th.fg));
        let indent = format!("{INDENT}{INDENT}");
        if self.mysql {
            out.push((format!("{indent}{}", self.label(Label::MonitorReplicaMysqlLag)), dim));
            return;
        }
        if rep.visible == Visibility::Hidden {
            out.push((format!("{indent}{}", self.label(Label::MonitorReplStreamHidden)), dim));
            return;
        }
        let (lag, style) = self.lag(&rep.lag(r.position), false);
        let mut parts = vec![lag];
        let times: Vec<String> = [
            (Label::MonitorReplicaWrite, rep.write_lag),
            (Label::MonitorReplicaFlush, rep.flush_lag),
            (Label::MonitorReplicaReplay, rep.replay_lag),
        ]
        .iter()
        .filter_map(|(l, v)| v.map(|v| format!("{} {}", self.label(*l), age(v))))
        .collect();
        if !times.is_empty() {
            parts.push(self.msg(Msg::MonitorReplicaLags { lags: times.join(", ") }));
        }
        if let Some(a) = rep.reply.and_then(|t| r.time.since(t)) {
            parts.push(self.msg(Msg::MonitorReplicaReply { time: age(a) }));
        }
        pack(out, &indent, parts, w, style);
    }

    /// A slot: what it is, whether its consumer is there, the log it keeps.
    fn slot(&self, s: &Slot) -> (Vec<String>, Style) {
        let th = theme::cur();
        let mut parts = vec![s.name.clone()];
        parts.push(match (&s.plugin, &s.database) {
            (Some(plugin), Some(db)) => format!("{} ({plugin}, {db})", s.kind),
            (Some(plugin), None) => format!("{} ({plugin})", s.kind),
            _ => s.kind.clone(),
        });
        if s.temporary {
            parts.push(self.label(Label::MonitorSlotTemporary));
        }
        parts.push(match (s.active, s.active_pid) {
            (true, Some(pid)) => self.msg(Msg::MonitorSlotInUseBy { pid: pid.to_string() }),
            (true, None) => self.label(Label::MonitorSlotInUse),
            (false, _) => self.label(Label::MonitorSlotNotInUse),
        });
        if let Some(kept) = s.kept {
            parts.push(self.msg(Msg::MonitorSlotKeeps { bytes: human_bytes(kept) }));
        }
        if let Some(status) = &s.wal_status {
            parts.push(format!("wal_status {status}"));
        }
        if let Some(safe) = s.safe_wal_size {
            parts.push(self.msg(Msg::MonitorSlotSafe { bytes: human_bytes(safe.max(0) as u64) }));
        }
        // A slot whose consumer is away keeps log that grows; a lost one cannot be used again.
        let style = match s.wal_status.as_deref() {
            Some("lost") => Style::new().fg(th.error),
            Some("unreserved") => Style::new().fg(th.warning),
            _ if !s.active && s.kept.is_some_and(|k| k > 0) => Style::new().fg(th.warning),
            _ => Style::new().fg(th.fg),
        };
        (parts, style)
    }

    fn member(&self, member: &Member) -> (Vec<String>, Style) {
        let th = theme::cur();
        let mut parts = vec![member.address.clone(), member.state.clone()];
        parts.extend(member.role.clone());
        if member.this {
            parts.push(self.label(Label::MonitorReplThisServer));
        }
        let style = match member.state.as_str() {
            "ONLINE" => Style::new().fg(th.fg),
            "RECOVERING" => Style::new().fg(th.warning),
            _ => Style::new().fg(th.error),
        };
        (parts, style)
    }

    /// What the server is, in full, over `area` (from line `top`, kept within the lines): the
    /// top drawn and the lines on screen.
    pub(super) fn server_info(&self, top: usize, close_key: &str, area: Rect, buf: &mut Buffer) -> (usize, u16) {
        let th = theme::cur();
        if area.height < 3 || area.width < 12 {
            return (top, 0);
        }
        let w = area.width.saturating_sub(4) as usize;
        let lines = self.server_lines(w, close_key);
        let height = (lines.len() as u16 + 2).min(area.height);
        let box_area = Rect { x: area.x, y: area.y, width: area.width, height };
        Clear.render(box_area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(th.accent))
            .style(Style::new().bg(th.surface))
            .title(format!(" {} ", self.label(Label::MonitorServer)));
        let inner = block.inner(box_area);
        block.render(box_area, buf);
        (draw_lines(&lines, top, inner, buf), inner.height)
    }

    fn server_lines(&self, w: usize, close_key: &str) -> Lines {
        let th = theme::cur();
        let text = Style::new().fg(th.fg);
        let dim = Style::new().fg(th.fg_dim);
        let mut fields: Vec<(String, String, Style)> = Vec::new();
        let Some(s) = &self.m.server else {
            return vec![(self.label(Label::MonitorReading), dim)];
        };
        fields.push((self.label(Label::MonitorServerVersion), s.version.clone(), text));
        let provider = match self.m.format().and_then(|f| f.flavour().product()) {
            Some(p) => p.to_string(),
            None => self.label(Label::MonitorServerNoProvider),
        };
        fields.push((self.label(Label::MonitorServerProvider), provider, text));
        let role = self.label(if s.read_only { Label::MonitorReadOnly } else { Label::MonitorReadWrite });
        fields.push((self.label(Label::MonitorServerRole), role, text));
        let now = self.m.snapshot.as_ref().map_or(s.read_at, |r| r.sample.time);
        if let Some(up) = s.started.and_then(|t| now.since(t)) {
            fields.push((self.label(Label::MonitorServerStarted), self.msg(Msg::MonitorAgo { time: age(up) }), text));
        }
        let gauge =
            |kind| self.m.snapshot.as_ref().and_then(|r| r.gauges.iter().find(|g| g.kind == kind)).map(|g| g.value);
        let connected = gauge(GaugeKind::Sessions).map(|n| n.to_string());
        let connections = match (connected, s.max_connections) {
            (Some(now), Some(max)) => Some(self.msg(Msg::MonitorServerConnectionsOf { now, max: max.to_string() })),
            (None, Some(max)) => Some(self.msg(Msg::MonitorMaxConnections { n: max.to_string() })),
            (Some(now), None) => Some(now),
            (None, None) => None,
        };
        if let Some(mut c) = connections {
            // Sessions the account cannot tell are clients are not in the count: said apart.
            if let Some(n) = gauge(GaugeKind::Hidden).filter(|n| *n > 0) {
                c.push_str(&format!(" · {n} {}", self.label(Label::MonitorCountHidden)));
            }
            fields.push((self.label(Label::MonitorServerConnections), c, text));
        }
        if let Some(b) = s.cache_bytes {
            fields.push((self.label(Label::MonitorServerCache), human_bytes(b), text));
        }
        for e in &s.extra {
            // A setting that leaves the monitor less to read says so.
            let off = matches!(e.value.as_str(), "off" | "OFF" | "0");
            let warning = match e.key {
                ExtraKey::TrackActivities if off => Some(Label::MonitorSettingNoActivities),
                ExtraKey::TrackCounts if off => Some(Label::MonitorSettingNoCounts),
                ExtraKey::PerformanceSchema if off => Some(Label::MonitorSettingNoPerformanceSchema),
                _ => None,
            };
            match warning {
                Some(l) => fields.push((
                    e.key.name().to_string(),
                    format!("{} ({})", e.value, self.label(l)),
                    Style::new().fg(th.warning),
                )),
                None => fields.push((e.key.name().to_string(), e.value.clone(), text)),
            }
        }
        // Keys up to [`KEY_MAX`] columns make the key column; a longer one (a setting's name)
        // has its value on the line after it, so a narrow screen keeps room for the values.
        let key_w = fields.iter().map(|(k, _, _)| width(k)).filter(|w| *w <= KEY_MAX).max().unwrap_or(0) + 2;
        let room = w.saturating_sub(key_w).max(10);
        let mut out = Lines::new();
        for (k, v, style) in fields {
            let lines = crate::text::wrap_words(&v, room);
            let mut key = Some(k);
            if key.as_deref().is_some_and(|k| width(k) + 2 > key_w) {
                out.push((key.take().unwrap_or_default(), style));
            }
            for l in lines {
                // Padded by display width: a key may hold wide characters.
                let k = key.take().unwrap_or_default();
                let pad = " ".repeat(key_w.saturating_sub(width(&k)));
                out.push((format!("{k}{pad}{l}"), style));
            }
        }
        out.push((String::new(), text));
        out.push((self.msg(Msg::MonitorServerClose { key: close_key.to_string() }), dim));
        out
    }
}

/// `lines` from line `top` in `area` (`top` kept within the lines so the last line can reach
/// the bottom): the top drawn.
fn draw_lines(lines: &Lines, top: usize, area: Rect, buf: &mut Buffer) -> usize {
    let height = area.height as usize;
    let top = top.min(lines.len().saturating_sub(height));
    let w = area.width.saturating_sub(2) as usize;
    for (i, (text, style)) in lines.iter().skip(top).take(height).enumerate() {
        put(buf, area.x + 1, area.y + i as u16, text, w, *style);
    }
    super::work(lines.len().min(height) as u64);
    top
}

/// `text` wrapped on words to `w` columns, each line set in by `indent`, in `style`.
fn wrapped(out: &mut Lines, indent: &str, text: &str, w: usize, style: Style) {
    for l in crate::text::wrap_words(text, w.saturating_sub(width(indent)).max(10)) {
        out.push((format!("{indent}{l}"), style));
    }
}

/// `parts` joined by ` · ` on lines of at most `w` columns, set in by `indent` (a line that
/// goes on, by one step more), in `style`: a part goes whole on a line where it fits, and one
/// longer than a line is wrapped on its words.
fn pack(out: &mut Lines, indent: &str, parts: Vec<String>, w: usize, style: Style) {
    let more = format!("{indent}{INDENT}");
    let mut line = String::new();
    for part in parts {
        if line.is_empty() {
            line = indent.to_owned();
        } else if width(&line) + 3 + width(&part) <= w {
            line.push_str(" · ");
            line.push_str(&part);
            continue;
        } else {
            out.push((std::mem::take(&mut line), style));
            line = more.clone();
        }
        // Every piece fits a line that goes on (set in further than the first).
        let room = w.saturating_sub(width(&more)).max(10);
        let mut pieces = crate::text::wrap_words(&part, room).into_iter();
        line.push_str(&pieces.next().unwrap_or_default());
        for piece in pieces {
            out.push((std::mem::take(&mut line), style));
            line = format!("{more}{piece}");
        }
    }
    if !line.is_empty() {
        out.push((line, style));
    }
}

/// The style of a side of replication by whether it runs.
fn run_style(r: &Running) -> Style {
    let th = theme::cur();
    match r {
        Running::Yes => Style::new().fg(th.fg),
        Running::No => Style::new().fg(th.error),
        Running::Connecting | Running::Paused | Running::Other(_) => Style::new().fg(th.warning),
        Running::Unknown => Style::new().fg(th.fg_dim),
    }
}

/// The most characters of a GTID set shown.
const SET_CHARS: usize = 600;

/// The start of a GTID set on one line, cut with `…` past [`SET_CHARS`] characters.
fn set_start(set: &str) -> String {
    let start: String = set.chars().take(SET_CHARS + 1).collect();
    let mut line = one_line(&start);
    if start.chars().count() > SET_CHARS {
        line = line.chars().take(SET_CHARS).collect();
        line.push('…');
    }
    line
}

/// Text on one line: its whitespace runs as one space (GTID sets and errors come over several).
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_gtid_set_shows_its_start_only() {
        let set: String = (0..100_000).map(|i| format!("{i}-{i},\n")).collect();
        let shown = set_start(&set);
        assert_eq!(shown.chars().count(), SET_CHARS + 1);
        assert!(shown.ends_with('…') && !shown.contains('\n'));
        assert_eq!(set_start("a:1-5,\nb:1-3"), "a:1-5, b:1-3");
    }
}
