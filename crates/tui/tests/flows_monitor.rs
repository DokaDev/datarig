//! The Monitor tab: `:monitor` (`M` in the explorer, the connection's menu) opens one per
//! profile, which opens a monitor connection of its own once the profile is connected and reads
//! only while it is the active tab: one read at a time, the next one an interval after the
//! answer; a hidden tab sends nothing. Answers of an older read or connection are dropped. A
//! fake driver records what is sent and the test feeds the answers.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbError, DbEvent, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::monitor::format::{Flavour, PgMonitor, Route};
use datarig_core::monitor::{
    Counter, CounterKind, Epoch, FullText, Gauge, GaugeKind, Hidden, MonitorFormat, Need, Own, Part, Proof, QueryText,
    Sample, ServerInfo, SessionKey, SessionKind, SessionList, SessionRow, SessionState, Snapshot, StatementStamp,
    TextAsk, Timestamp, Tx, Visibility, Wait, WaitClass,
};
use datarig_tui::app::monitor::{MonState, MonitorView};
use datarig_tui::app::{Focus, TabKind};
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Seconds as server time (microseconds since the epoch), from a fixed start.
fn at(secs: f64) -> Timestamp {
    Timestamp(1_794_060_000_000_000 + (secs * 1_000_000.0) as i64)
}

fn pg_format() -> MonitorFormat {
    MonitorFormat::Postgres(PgMonitor {
        version: 170011,
        superuser: true,
        read_all_stats: true,
        signal_backend: true,
        flavour: Flavour::Standard,
        route: Route::Direct,
    })
}

fn session(id: i64, user: &str, state: SessionState, query_secs: f64, query: &str) -> SessionRow {
    SessionRow {
        key: SessionKey { id, started: Some(at(-3600.0)) },
        kind: SessionKind::Client,
        user: Some(user.into()),
        database: Some("shop".into()),
        application: Some("app".into()),
        client: Some("10.0.0.7:51022".into()),
        state,
        wait: None,
        tx: Tx::Since(at(-query_secs - 1.0)),
        query_started: Some(at(-query_secs)),
        state_since: Some(at(-query_secs)),
        query: Some(QueryText { text: query.into(), shortened: false, maybe_cut_by_server: false }),
        blocked_by: None,
        monitor: false,
        visible: Visibility::Full,
        extra: Vec::new(),
        thread: None,
        statement: Some(StatementStamp::Pg { query_start: Some(at(-query_secs)) }),
    }
}

/// A read at `secs` with `commits` so far and the given sessions.
fn snapshot(secs: f64, commits: u64, rows: Vec<SessionRow>, server: bool) -> Snapshot {
    let time = at(secs);
    // Ages are worked out against the read's own clock: rows made relative to 0 keep their age.
    let rows = rows
        .into_iter()
        .map(|mut r| {
            let shift = |t: Timestamp| Timestamp(t.0 + (secs * 1_000_000.0) as i64);
            r.query_started = r.query_started.map(shift);
            r.state_since = r.state_since.map(shift);
            if let Tx::Since(t) = r.tx {
                r.tx = Tx::Since(shift(t));
            }
            r
        })
        .collect::<Vec<_>>();
    let active = rows.iter().filter(|r| r.state == SessionState::Active).count() as u64;
    Snapshot {
        sample: Sample {
            time,
            epoch: Epoch::Mark(1, 3),
            counters: vec![
                Counter { kind: CounterKind::Commits, value: commits },
                Counter { kind: CounterKind::Rollbacks, value: 7 },
                Counter { kind: CounterKind::RowsRead, value: commits * 20 },
                Counter { kind: CounterKind::RowsWritten, value: commits * 2 },
                Counter { kind: CounterKind::PageRequests, value: commits * 100 },
                Counter { kind: CounterKind::PageReads, value: commits },
                Counter { kind: CounterKind::RowsInserted, value: commits },
                Counter { kind: CounterKind::RowsUpdated, value: commits / 2 },
                Counter { kind: CounterKind::RowsDeleted, value: commits / 2 },
            ],
            own: Own::default(),
        },
        gauges: vec![
            Gauge { kind: GaugeKind::Sessions, value: rows.len() as u64 + 37 },
            Gauge { kind: GaugeKind::Active, value: active },
            Gauge { kind: GaugeKind::IdleInTx, value: 1 },
            Gauge { kind: GaugeKind::Idle, value: 37 },
            Gauge { kind: GaugeKind::WaitingForLock, value: 1 },
            Gauge { kind: GaugeKind::Hidden, value: 0 },
            // By state, each once: the quick select runs, the update waits, the monitor's own
            // is left out.
            Gauge { kind: GaugeKind::StateRunning, value: 1 },
            Gauge { kind: GaugeKind::StateWaiting, value: 1 },
            Gauge { kind: GaugeKind::StateIdleInTx, value: 1 },
            Gauge { kind: GaugeKind::StateIdle, value: 37 },
            Gauge { kind: GaugeKind::StateBackground, value: 5 },
            Gauge { kind: GaugeKind::StateUnknown, value: 0 },
        ],
        locks: None,
        sessions: Some(SessionList { time, rows, idle_left_out: true }),
        server: server.then(|| ServerInfo {
            version: "PostgreSQL 17.11 (Debian 17.11-1.pgdg13+2) on aarch64-unknown-linux-gnu".into(),
            started: Some(Timestamp(time.0 - (41 * 86_400 + 3 * 3600) * 1_000_000)),
            read_at: time,
            read_only: false,
            max_connections: Some(200),
            cache_bytes: Some(134_217_728),
            extra: Vec::new(),
        }),
        hidden: Vec::new(),
    }
}

/// The usual sessions: a long update waiting for a lock, an idle transaction, a quick select.
fn rows() -> Vec<SessionRow> {
    let mut waiting =
        session(4211, "alice", SessionState::Active, 190.0, "UPDATE orders SET status = 'paid' WHERE id = 7");
    waiting.wait = Some(Wait { class: WaitClass::Lock, name: "transactionid".into() });
    let mut idle_tx =
        session(3907, "bob", SessionState::IdleInTx, 720.0, "SELECT * FROM orders WHERE id = 7 FOR UPDATE");
    idle_tx.state_since = Some(at(-700.0));
    let quick = session(4302, "web", SessionState::Active, 0.4, "SELECT count(*) FROM order_items");
    let mut own = session(4400, "datarig", SessionState::Active, 0.0, "SELECT … FROM pg_stat_activity");
    own.monitor = true;
    vec![waiting, idle_tx, quick, own]
}

/// The monitor reads that went out since the last look: their request ids and whether they
/// asked for idle sessions.
fn ticks(h: &mut Harness) -> Vec<(u64, bool, bool)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::MonitorTick { request, want } => {
                Some((request, want.sessions.is_some_and(|s| s.idle), want.server))
            }
            _ => None,
        })
        .collect()
}

/// Connected, with a driver that monitors; the Monitor tab open (`:monitor`) and its
/// connection probed, its first read sent and not answered.
fn monitor_open() -> (Harness, u64) {
    let mut h = Harness::connected(Lang::En);
    h.driver.monitor.store(true, Ordering::SeqCst);
    h.sent();
    h.command("monitor");
    assert_eq!(h.app.tab().kind, TabKind::Monitor);
    assert_eq!(h.roles().last(), Some(&SessionRole::Monitor), "{:?}", h.roles());
    // Nothing is read before the probe answers.
    assert!(ticks(&mut h).is_empty());
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(pg_format())));
    let sent = ticks(&mut h);
    assert_eq!(sent.len(), 1, "one read at once: {sent:?}");
    assert!(sent[0].2, "the first read asks what the server is");
    (h, sent[0].0)
}

fn monitor(h: &Harness) -> &datarig_tui::app::monitor::MonitorTab {
    h.app.monitor_of(h.app.tab().id).expect("a Monitor tab")
}

#[test]
fn opens_once_per_profile_and_reads_one_at_a_time_an_interval_after_each_answer() {
    let (mut h, first) = monitor_open();
    assert_eq!(h.app.focus, Focus::Results);
    // Nothing more while the read is out, however long it takes.
    h.advance(Duration::from_secs(4));
    assert!(ticks(&mut h).is_empty());
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    // The next one an interval (2 s) after the answer, not before.
    h.advance(Duration::from_millis(1900));
    assert!(ticks(&mut h).is_empty());
    h.advance(Duration::from_millis(200));
    let next = ticks(&mut h);
    assert_eq!(next.len(), 1);
    assert!(!next[0].2, "what the server is was read with the first");
    h.monitor_db(DbEvent::MonitorSnapshot { request: next[0].0, result: Ok(snapshot(2.0, 1824, rows(), false)) });
    // (1824 - 1000) / 2 s.
    let commits = monitor(&h).history_of(datarig_tui::app::monitor::Metric::Rate(CounterKind::Commits)).unwrap();
    assert_eq!(commits.last(), Some(412.0));
    // Opening it again goes to the same tab; no second connection.
    let tabs = h.app.tabs.len();
    let sessions = h.roles().len();
    h.command("monitor local-pg");
    assert_eq!((h.app.tabs.len(), h.roles().len()), (tabs, sessions));
}

#[test]
fn a_hidden_monitor_tab_reads_nothing_and_shows_a_gap_when_shown_again() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    let monitor_tab = h.app.tab().id;
    // Another tab.
    h.app.focus = Focus::Results;
    h.key_mod(KeyCode::PageUp, ratatui::crossterm::event::KeyModifiers::CONTROL);
    assert_ne!(h.app.tab().id, monitor_tab);
    // Nothing is planned for it: no wake-up, no read, however long.
    assert_eq!(h.app.monitor_due(), None);
    for _ in 0..30 {
        h.advance(Duration::from_secs(2));
    }
    assert!(ticks(&mut h).is_empty(), "a hidden Monitor tab reads nothing");
    // Shown again: a read at once; the time it was hidden is a gap, not a line.
    h.key_mod(KeyCode::PageDown, ratatui::crossterm::event::KeyModifiers::CONTROL);
    assert_eq!(h.app.tab().id, monitor_tab);
    let again = ticks(&mut h);
    assert_eq!(again.len(), 1);
    h.monitor_db(DbEvent::MonitorSnapshot { request: again[0].0, result: Ok(snapshot(62.0, 9000, rows(), false)) });
    let commits = monitor(&h).history_of(datarig_tui::app::monitor::Metric::Rate(CounterKind::Commits)).unwrap();
    let points: Vec<Option<f64>> = commits.points().map(|(_, v)| v).collect();
    assert_eq!(points, [None, None, None], "the first point, the gap, the point after it: no rate across");
}

#[test]
fn answers_of_an_older_read_or_connection_are_dropped() {
    let (mut h, first) = monitor_open();
    // Not the read in flight.
    h.monitor_db(DbEvent::MonitorSnapshot { request: first + 7, result: Ok(snapshot(0.0, 1, rows(), true)) });
    assert!(monitor(&h).snapshot.is_none());
    // The connection is lost: it opens again at the next read, and the old one's answer is
    // dropped (its generation).
    let old = monitor(&h).generation;
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    assert!(matches!(monitor(&h).state, MonState::Lost(_)));
    h.advance(Duration::from_secs(3));
    assert_ne!(monitor(&h).generation, old, "a new connection");
    assert_eq!(h.roles().iter().filter(|r| **r == SessionRole::Monitor).count(), 2);
    let id = h.app.tab().id;
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target: datarig_tui::app::EventTarget::Monitor(id),
        generation: old,
        ev: DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1, rows(), true)) },
    });
    assert!(monitor(&h).snapshot.is_none(), "an older connection's answer is dropped");
}

#[test]
fn pause_stops_reading_r_reads_once_and_failures_slow_the_reads_down() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.keys("p");
    assert!(monitor(&h).paused);
    h.advance(Duration::from_secs(10));
    assert!(ticks(&mut h).is_empty());
    // `r` reads once and stays paused.
    h.keys("r");
    let once = ticks(&mut h);
    assert_eq!(once.len(), 1);
    assert!(monitor(&h).paused);
    h.monitor_db(DbEvent::MonitorSnapshot { request: once[0].0, result: Ok(snapshot(12.0, 1100, rows(), true)) });
    h.advance(Duration::from_secs(10));
    assert!(ticks(&mut h).is_empty());
    h.keys("p");
    // Three failed reads in a row: the delay doubles.
    for n in 0..3 {
        let t = ticks(&mut h);
        assert_eq!(t.len(), 1, "read {n}");
        h.monitor_db(DbEvent::MonitorSnapshot {
            request: t[0].0,
            result: Err(DbError::Server("canceling statement due to statement timeout".into())),
        });
        h.advance(Duration::from_secs(2));
    }
    assert!(ticks(&mut h).is_empty(), "the fourth read waits 4 s");
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1);
    assert!(monitor(&h).error.is_some());
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(30.0, 2000, rows(), false)) });
    assert_eq!(monitor(&h).failures, 0);
    assert!(monitor(&h).error.is_none());
    assert_eq!(monitor(&h).delay(), Duration::from_secs(2));
}

#[test]
fn idle_sessions_are_asked_for_with_a_and_the_interval_follows_plus_and_minus() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.keys("a");
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1, "read again at once");
    assert!(t[0].1, "with idle sessions");
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(1.0, 1001, rows(), false)) });
    h.keys("-");
    assert_eq!(monitor(&h).interval.as_str(), "5s");
    h.keys("++");
    assert_eq!(monitor(&h).interval.as_str(), "1s");
}

#[test]
fn the_sessions_view_sorts_filters_and_opens_a_statement_in_a_console_without_running_it() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.keys("2");
    assert_eq!(monitor(&h).view, MonitorView::Sessions);
    // Longest statement first.
    let ids = |h: &Harness| {
        let m = monitor(h);
        let list = &m.snapshot.as_ref().unwrap().sessions.as_ref().unwrap().rows;
        m.list.order.iter().map(|&i| list[i].key.id).collect::<Vec<_>>()
    };
    assert_eq!(ids(&h), [3907, 4211, 4302, 4400]);
    h.keys("/");
    h.type_text("orders");
    h.key(KeyCode::Enter);
    assert_eq!(ids(&h), [3907, 4211]);
    h.keys("j");
    assert_eq!(monitor(&h).selected_row().unwrap().key.id, 4211);
    // The selection stays on its session across reads.
    let t = {
        h.advance(Duration::from_secs(2));
        ticks(&mut h)
    };
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(2.0, 1010, rows(), false)) });
    assert_eq!(monitor(&h).selected_row().unwrap().key.id, 4211);
    let tabs = h.app.tabs.len();
    h.keys("o");
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert_eq!(h.app.tab().kind, TabKind::Console);
    assert_eq!(h.app.tab().editor.text(), "UPDATE orders SET status = 'paid' WHERE id = 7");
    assert_eq!(h.app.tab().context.database.as_deref(), Some("shop"));
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::Execute { .. })), "the statement is never run");
}

#[test]
fn what_the_account_cannot_see_is_said_with_what_would_show_it() {
    let (mut h, first) = monitor_open();
    let mut s = snapshot(0.0, 1000, rows(), true);
    s.hidden.push(Hidden { part: Part::OtherSessionDetails, need: Need::Grant("pg_read_all_stats") });
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(s) });
    let screen = h.screen(160, 40);
    assert!(
        screen.contains(
            "Other accounts' sessions are listed without their state, statement and times (needs pg_read_all_stats)"
        ),
        "{screen}"
    );
}

#[test]
fn a_driver_without_a_monitor_says_so() {
    let mut h = Harness::connected(Lang::En);
    let tabs = h.app.tabs.len();
    h.command("monitor");
    assert_eq!(h.app.tabs.len(), tabs);
    assert!(h.status(120, 40).contains("This connection's driver has no server monitor"));
}

#[test]
fn screens_80x24_and_120x40() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(2.0, 1824, rows(), false)) });
    let screen = h.screen(120, 40);
    // The rows by operation are not parts of one total: none on the border.
    let head = screen.lines().find(|l| l.contains("┌ rows/s by operation")).unwrap();
    let border = &head[head.find("┌ rows/s by operation").unwrap()..];
    assert!(!border[..border.find('┐').unwrap()].chars().any(|c| c.is_ascii_digit()), "{screen}");
    insta::assert_snapshot!("monitor_overview_120x40", screen);
    insta::assert_snapshot!("monitor_overview_80x24", h.screen(80, 24));
    h.keys("2");
    insta::assert_snapshot!("monitor_sessions_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_sessions_80x24", h.screen(80, 24));
    h.keys("j");
    h.key(KeyCode::Enter);
    insta::assert_snapshot!("monitor_details_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_details_80x24", h.screen(80, 24));
}

/// A MySQL read at `secs` with `n` so far (statements, `SELECT`s and the rest grow with it) and
/// the usual sessions as MySQL lists them.
fn mysql_snapshot(secs: f64, n: u64, server: bool) -> Snapshot {
    let mut s = snapshot(secs, n, mysql_rows("SELECT count(*) FROM order_items"), false);
    s.sample.epoch = Epoch::Uptime(57_000 + secs as u64);
    s.sample.counters = [
        (CounterKind::Statements, n * 2),
        (CounterKind::ExplicitCommits, n / 10),
        (CounterKind::ExplicitRollbacks, 2),
        (CounterKind::RowsRead, n * 20),
        (CounterKind::RowsWritten, n / 2),
        (CounterKind::PageRequests, n * 100),
        (CounterKind::PageReads, n),
        (CounterKind::BytesIn, n * 300),
        (CounterKind::BytesOut, n * 900),
        (CounterKind::RowLockWaits, 7),
        (CounterKind::SlowQueries, 1),
        (CounterKind::TempDiskTables, 118),
        (CounterKind::AbortedConnects, 0),
        (CounterKind::Selects, n * 2 - n / 4),
        (CounterKind::Inserts, n / 8),
        (CounterKind::Updates, n / 16),
        (CounterKind::Deletes, n / 64),
    ]
    .map(|(kind, value)| Counter { kind, value })
    .to_vec();
    s.gauges = [
        (GaugeKind::Sessions, 47),
        (GaugeKind::Active, 2),
        (GaugeKind::CommandQuery, 2),
        (GaugeKind::CommandSleep, 43),
        (GaugeKind::CommandDaemon, 1),
        (GaugeKind::CommandConnect, 0),
        (GaugeKind::CommandBinlogDump, 1),
        (GaugeKind::CommandOther, 0),
    ]
    .map(|(kind, value)| Gauge { kind, value })
    .to_vec();
    s.server = server.then(|| ServerInfo {
        version: "8.4.11 MySQL Community Server - GPL".into(),
        started: Some(Timestamp(s.sample.time.0 - 57_000 * 1_000_000)),
        read_at: s.sample.time,
        read_only: false,
        max_connections: Some(151),
        cache_bytes: Some(134_217_728),
        extra: Vec::new(),
    });
    s
}

/// A Monitor tab on a MySQL server, its first read answered with `first`.
fn mysql_open(first: Snapshot) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.driver.monitor.store(true, Ordering::SeqCst);
    h.command("monitor");
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(mysql_format())));
    let t = ticks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(first) });
    h
}

/// The next read, answered with `s`.
fn answer_next(h: &mut Harness, s: Snapshot) {
    h.advance(Duration::from_secs(2));
    let t = ticks(h);
    assert_eq!(t.len(), 1, "{t:?}");
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(s) });
}

#[test]
fn mysql_overview_screens_80x24_and_120x40() {
    use datarig_tui::app::monitor::Metric;
    let mut h = mysql_open(mysql_snapshot(0.0, 1000, true));
    // One read alone has no rate: the series start with a gap, never 0.
    let selects = Metric::Rate(CounterKind::Selects);
    assert_eq!(monitor(&h).history_of(selects).unwrap().last(), None);
    let sleeping = Metric::Gauge(GaugeKind::CommandSleep);
    assert_eq!(monitor(&h).history_of(sleeping).unwrap().last(), Some(43.0), "a count needs no pair");
    // A load that grows and falls back, so the lines move.
    let mut n = 1000;
    for (i, step) in [400, 900, 1600, 1200, 700, 1824 - 1000].into_iter().enumerate() {
        n += step;
        answer_next(&mut h, mysql_snapshot(2.0 * (i + 1) as f64, n, false));
    }
    assert_eq!(monitor(&h).history_of(selects).unwrap().last(), Some(721.0));
    let screen = h.screen(120, 40);
    assert!(screen.contains("statements/s by type") && screen.contains("sessions by state"), "{screen}");
    assert!(screen.contains("active 2"), "the Sessions tile keeps its text: {screen}");
    insta::assert_snapshot!("monitor_overview_mysql_120x40", screen);
    insta::assert_snapshot!("monitor_overview_mysql_80x24", h.screen(80, 24));
}

#[test]
fn the_charts_wrap_their_legend_and_give_way_when_short() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    answer_next(&mut h, snapshot(2.0, 1824, rows(), false));
    // Too narrow for two side by side: one under the other, a legend on two rows.
    let stacked = h.screen(88, 40);
    let line_of = |screen: &str, text: &str| screen.lines().position(|l| l.contains(text));
    let (running, background) = (line_of(&stacked, "■ running 1"), line_of(&stacked, "■ background 5"));
    assert!(running.is_some() && background > running, "the legend wraps, no series cut: {stacked}");
    insta::assert_snapshot!("monitor_overview_stacked_88x40", stacked);
    // Narrower and shorter: both legends wrap, so the second chart is a line of its own.
    let short = h.screen(80, 30);
    let (first, second) = (line_of(&short, "┌ rows/s by operation"), line_of(&short, "sessions by state  45"));
    assert!(first.is_some() && second > first, "{short}");
    insta::assert_snapshot!("monitor_overview_short_80x30", short);
    // Wide but short: a list of lines, each chart a line of its own.
    let lines = h.screen(120, 26);
    let row = |text: &str| lines.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("{text}: {lines}"));
    let column = |line: &str, text: &str| line.find(text).map(|b| line[..b].chars().count());
    let (commits, rows, states) = (row("commits/s"), row("rows/s by operation"), row("sessions by state"));
    assert!(rows.contains("■ read 8.2 k  ■ inserted 412"), "{lines}");
    // One column of values for metrics and charts alike; the rows by operation have no total.
    assert_eq!(column(commits, "412"), column(states, "45"), "{lines}");
    assert_eq!(column(rows, "■ read"), column(states, "■ running"), "{lines}");
    insta::assert_snapshot!("monitor_overview_lines_120x26", lines);
}

#[test]
fn a_series_the_server_starts_or_stops_giving_starts_or_breaks_there() {
    use datarig_tui::app::monitor::Metric;
    let inserted = Metric::Rate(CounterKind::RowsInserted);
    let off = |mut s: Snapshot| {
        s.sample.counters.retain(|c| {
            !matches!(c.kind, CounterKind::RowsInserted | CounterKind::RowsUpdated | CounterKind::RowsDeleted)
        });
        s
    };
    // `track_counts` off when the tab opens: the chart is not there.
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(off(snapshot(0.0, 1000, rows(), true))) });
    answer_next(&mut h, off(snapshot(2.0, 1824, rows(), false)));
    assert!(monitor(&h).history_of(inserted).is_none());
    assert!(h.screen(120, 40).contains("rows/s by operation"));
    assert!(h.screen(120, 40).contains("not available"));
    // Turned on: the series starts at that read, with no rate yet (one sample) and nothing
    // filled in before it; the next read has its rate.
    answer_next(&mut h, snapshot(4.0, 2648, rows(), false));
    let points: Vec<_> = monitor(&h).history_of(inserted).unwrap().points().map(|(_, v)| v).collect();
    assert_eq!(points, [None], "it starts here");
    answer_next(&mut h, snapshot(6.0, 3472, rows(), false));
    assert_eq!(monitor(&h).history_of(inserted).unwrap().last(), Some(412.0));
    let metrics: Vec<Metric> = monitor(&h).history.iter().map(|(m, _)| *m).collect();
    let order: Vec<Metric> = datarig_tui::app::monitor::METRICS
        .into_iter()
        .chain(datarig_tui::app::monitor::SERIES)
        .filter(|m| metrics.contains(m))
        .collect();
    assert_eq!(metrics, order, "the histories keep their order");
    let screen = h.screen(120, 40);
    assert!(screen.contains("■ inserted 412") && !screen.contains("not available"), "{screen}");
    // Turned off again: a gap from then on, never its last value or zero.
    answer_next(&mut h, off(snapshot(8.0, 4296, rows(), false)));
    assert_eq!(monitor(&h).history_of(inserted).unwrap().last(), None);
    let screen = h.screen(120, 40);
    assert!(screen.contains("■ inserted –") && screen.contains("■ deleted –"), "{screen}");
}

#[test]
fn a_chart_says_why_it_is_not_there_and_whose_threads_it_counts() {
    // `track_counts` off: no rows by operation, said so, not 0/s.
    let (mut h, first) = monitor_open();
    let off = |mut s: Snapshot| {
        s.sample.counters.retain(|c| {
            !matches!(c.kind, CounterKind::RowsInserted | CounterKind::RowsUpdated | CounterKind::RowsDeleted)
        });
        if let Some(info) = &mut s.server {
            info.extra.push(datarig_core::monitor::Extra {
                key: datarig_core::monitor::ExtraKey::TrackCounts,
                value: "off".into(),
            });
        }
        s
    };
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(off(snapshot(0.0, 1000, rows(), true))) });
    answer_next(&mut h, off(snapshot(2.0, 1824, rows(), false)));
    let screen = h.screen(120, 40);
    assert!(screen.contains("not available: track_counts = off"), "{screen}");
    // Without PROCESS the threads are the account's own.
    let mut own = mysql_snapshot(0.0, 1000, true);
    own.hidden.push(Hidden { part: Part::OtherSessions, need: Need::Grant("PROCESS") });
    let mut h = mysql_open(own);
    let screen = h.screen(120, 40);
    assert!(screen.contains("sessions by state: this account only"), "{screen}");
}

/// The theme with no chart colours; returns its text colour.
fn without_colours(h: &mut Harness) -> ratatui::style::Color {
    let mut plain = (*h.app.theme).clone();
    for c in [
        &mut plain.chart_1,
        &mut plain.chart_2,
        &mut plain.chart_3,
        &mut plain.chart_4,
        &mut plain.chart_5,
        &mut plain.chart_6,
    ] {
        *c = ratatui::style::Color::Reset;
    }
    let fg = plain.fg;
    h.app.theme = std::sync::Arc::new(plain);
    fg
}

/// The braille bits of the cells of the box whose title starts with `title`, row by row inside
/// its border.
fn chart_cells(buf: &ratatui::buffer::Buffer, title: &str) -> Vec<Vec<(u8, ratatui::style::Color)>> {
    let area = buf.area;
    let text = |y: u16| (0..area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<Vec<_>>();
    let head = format!("┌ {title}");
    for y in 0..area.height {
        let row = text(y);
        let line: String = row.concat();
        let Some(at) = line.find(&head) else { continue };
        let left = line[..at].chars().count() as u16;
        let right = (left + 1..area.width).find(|&x| buf[(x, y)].symbol() == "┐").expect("a box");
        return (y + 1..area.height)
            .take_while(|&r| buf[(left, r)].symbol() == "│")
            .map(|r| {
                (left + 1..right)
                    .map(|x| {
                        let c = &buf[(x, r)];
                        let ch = c.symbol().chars().next().unwrap_or(' ') as u32;
                        let bits = if (0x2800..=0x28FF).contains(&ch) { (ch - 0x2800) as u8 } else { 0 };
                        (bits, c.fg)
                    })
                    .collect()
            })
            .collect();
    }
    panic!("no chart {title}");
}

#[test]
fn a_total_is_unknown_while_one_of_its_series_has_no_value() {
    let mut h = mysql_open(mysql_snapshot(0.0, 1000, true));
    answer_next(&mut h, mysql_snapshot(2.0, 1824, false));
    // A CALL in between: our own SELECTs cannot be counted, so that point of the select line is
    // a gap, and so is the total: never insert + update + delete alone.
    let mut called = mysql_snapshot(4.0, 2648, false);
    called.sample.own.uncounted = 1;
    answer_next(&mut h, called);
    let screen = h.screen(120, 40);
    let head = screen.lines().find(|l| l.contains("┌ statements/s by type")).unwrap();
    let chart = &head[head.find("┌ statements/s by type").unwrap()..];
    assert!(chart[..chart.find('┐').unwrap()].ends_with(" – "), "{screen}");
    assert!(screen.contains("■ select –") && screen.contains("■ insert 52"), "{screen}");
    // Without colours the total line breaks there: nothing in the newest dot column.
    without_colours(&mut h);
    let term = h.draw(120, 40);
    let cells = chart_cells(term.backend().buffer(), "statements/s by type");
    const RIGHT_DOTS: u8 = 0x08 | 0x10 | 0x20 | 0x80;
    for row in cells.iter().take(5) {
        assert_eq!(row[row.len() - 1].0 & RIGHT_DOTS, 0, "{screen}");
    }
    assert!(cells.iter().take(5).any(|r| r.iter().any(|c| c.0 != 0)), "the points before are drawn");
}

#[test]
fn a_series_the_legend_cannot_name_is_not_drawn() {
    // Figures so wide that the threads' legend has no room left for its last entry.
    let wide = |mut s: Snapshot| {
        for g in &mut s.gauges {
            if matches!(g.kind, GaugeKind::CommandConnect | GaugeKind::CommandBinlogDump) {
                g.value = 123_456;
            }
        }
        s
    };
    let mut h = mysql_open(wide(mysql_snapshot(0.0, 1000, true)));
    answer_next(&mut h, wide(mysql_snapshot(2.0, 1824, false)));
    let screen = h.screen(120, 40);
    assert!(!screen.contains("■ other"), "{screen}");
    assert!(screen.lines().any(|l| l.contains("■ binlog dump 123456  …")), "{screen}");
    let palette = h.app.theme.chart();
    let term = h.draw(120, 40);
    let cells = chart_cells(term.backend().buffer(), "sessions by state");
    let colours: Vec<_> = cells.iter().flatten().filter(|c| c.0 != 0).map(|c| c.1).collect();
    assert!(!colours.contains(&palette[5]), "the other threads' line has no legend entry: {screen}");
    assert!(colours.contains(&palette[4]), "the binlog dump line is named: {screen}");
}

#[test]
fn without_colours_a_chart_draws_its_total_and_the_legend_tells_the_series() {
    use ratatui::style::Color;
    let mut h = mysql_open(mysql_snapshot(0.0, 1000, true));
    answer_next(&mut h, mysql_snapshot(2.0, 1824, false));
    let fg = without_colours(&mut h);
    let term = h.draw(120, 40);
    let buf = term.backend().buffer();
    let dots: Vec<Color> = buf
        .content()
        .iter()
        .filter(|c| c.symbol().chars().next().is_some_and(|ch| ('\u{2801}'..='\u{28FF}').contains(&ch)))
        .map(|c| c.fg)
        .collect();
    assert!(!dots.is_empty() && dots.iter().all(|c| *c == fg), "one line, the total: {dots:?}");
    let screen = h.screen(120, 40);
    for name in ["■ select 721", "■ insert 52", "■ query 2", "■ sleep 43"] {
        assert!(screen.contains(name), "{name}: {screen}");
    }
}

#[test]
fn a_monitor_tab_is_kept_with_the_workspace_and_reads_again_once_shown() {
    let (h, _) = monitor_open();
    let state = h.app.workspace_state();
    assert!(state.tabs.iter().any(|t| t.kind == datarig_core::workspace::TabKind::Monitor));
}

#[test]
fn a_disconnected_profile_stays_disconnected_until_r() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    let sessions = h.roles().len();
    h.keys(" cx");
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert!(!h.app.conns.is_connected(h.app.tab().profile.unwrap()), "disconnected");
    assert_eq!(monitor(&h).state, MonState::Closed);
    // Shown all along: nothing connects by itself, nothing is read.
    for _ in 0..10 {
        h.advance(Duration::from_secs(2));
    }
    assert!(h.connecting().is_none(), "the profile is not connected again by itself");
    assert_eq!(h.roles().len(), sessions);
    assert!(ticks(&mut h).is_empty());
    assert!(h.screen(120, 40).contains("Not connected: r connects"));
    // `r` connects it again.
    h.keys("r");
    assert!(h.connecting().is_some() || h.app.conns.is_connected(h.app.tab().profile.unwrap()));
}

#[test]
fn a_dialog_over_the_tab_plans_no_wakeup_for_it() {
    let (mut h, first) = monitor_open();
    // A dialog opens while the read is out; its answer plans the next read.
    h.keys(":");
    assert!(h.overlay_kind().is_some());
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    assert_eq!(h.app.monitor_due(), None, "covered: no deadline, no wake-ups");
    h.advance(Duration::from_secs(5));
    assert!(ticks(&mut h).is_empty());
    h.key(KeyCode::Esc);
    assert_eq!(ticks(&mut h).len(), 1, "uncovered: it reads at once");
}

#[test]
fn an_answer_that_came_while_hidden_is_no_base_for_the_next_rate() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    // Hidden while the read is out; its answer comes then.
    h.key_mod(KeyCode::PageUp, ratatui::crossterm::event::KeyModifiers::CONTROL);
    h.monitor_db_to(1, DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(2.0, 1100, rows(), false)) });
    h.advance(Duration::from_secs(60));
    h.key_mod(KeyCode::PageDown, ratatui::crossterm::event::KeyModifiers::CONTROL);
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(64.0, 9000, rows(), false)) });
    let commits = monitor(&h).history_of(datarig_tui::app::monitor::Metric::Rate(CounterKind::Commits)).unwrap();
    assert_eq!(commits.last(), None, "no rate across the hidden minute");
}

#[test]
fn r_while_paused_reads_once_even_when_the_connection_opens_again_first() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.keys("p");
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    h.keys("r");
    assert!(ticks(&mut h).is_empty(), "nothing to read on yet");
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(pg_format())));
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1, "the read r asked for");
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(10.0, 1100, rows(), true)) });
    assert!(monitor(&h).paused);
    h.advance(Duration::from_secs(10));
    assert!(ticks(&mut h).is_empty(), "still paused");
}

#[test]
fn a_probe_answer_queued_before_a_disconnect_is_dropped() {
    let mut h = Harness::connected(Lang::En);
    h.driver.monitor.store(true, Ordering::SeqCst);
    h.command("monitor");
    h.monitor_db(DbEvent::Connected);
    let id = h.app.tab().id;
    let old = monitor(&h).generation;
    h.keys(" cx");
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert_eq!(monitor(&h).state, MonState::Closed);
    // The old connection's probe answer, sent before it closed.
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target: datarig_tui::app::EventTarget::Monitor(id),
        generation: old,
        ev: DbEvent::Monitor(Ok(pg_format())),
    });
    assert_eq!(monitor(&h).state, MonState::Closed, "a closed connection's answer changes nothing");
    h.advance(Duration::from_secs(5));
    assert!(h.connecting().is_none(), "the profile stays disconnected");
}

#[test]
fn a_zoomed_explorer_hides_the_monitor_tab() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.app.tab_mut().pane.zoom = Some(Focus::Tree);
    h.app.focus = Focus::Tree;
    h.advance(Duration::from_secs(1));
    assert_eq!(h.app.monitor_due(), None);
    h.advance(Duration::from_secs(10));
    assert!(ticks(&mut h).is_empty(), "covered by the explorer: nothing read");
}

#[test]
fn what_the_server_is_is_asked_again_after_a_failed_first_read() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Err(DbError::Server("timeout".into())) });
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1);
    assert!(t[0].2, "the server info is asked for again");
}

#[test]
fn the_uptime_follows_the_latest_read() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    assert!(h.screen(120, 40).contains("up 41d3h"));
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(3600.0, 2000, rows(), false)) });
    assert!(h.screen(120, 40).contains("up 41d4h"), "an hour later by the server's clock");
}

#[test]
fn own_traffic_owed_before_a_gap_is_not_taken_from_the_load_after_it() {
    let (mut h, first) = monitor_open();
    let with_own = |s: Snapshot, own: u64| Snapshot {
        sample: Sample { own: Own { transactions: own, ..Own::default() }, ..s.sample },
        ..s
    };
    h.monitor_db(DbEvent::MonitorSnapshot {
        request: first,
        result: Ok(with_own(snapshot(0.0, 1000, rows(), true), 0)),
    });
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    // The monitor's own transaction not published yet: owed.
    h.monitor_db(DbEvent::MonitorSnapshot {
        request: t[0].0,
        result: Ok(with_own(snapshot(2.0, 1000, rows(), false), 1)),
    });
    h.keys("p");
    h.keys("p");
    let t = ticks(&mut h);
    // Published during the gap.
    h.monitor_db(DbEvent::MonitorSnapshot {
        request: t[0].0,
        result: Ok(with_own(snapshot(30.0, 1001, rows(), false), 2)),
    });
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot {
        request: t[0].0,
        result: Ok(with_own(snapshot(32.0, 1012, rows(), false), 3)),
    });
    let commits = monitor(&h).history_of(datarig_tui::app::monitor::Metric::Rate(CounterKind::Commits)).unwrap();
    assert_eq!(commits.last(), Some(5.0), "10 of the 11 are the load, over 2 s");
}

#[test]
fn a_disconnect_confirmed_in_a_dialog_stays_disconnected() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    // A dialog over the tab (as a disconnect's confirmation), then the disconnect.
    h.keys(":");
    let profile = h.app.tab().profile.unwrap();
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    if h.overlay_kind() == Some(datarig_tui::app::overlay::OverlayKind::Confirm) {
        h.keys("y");
    }
    h.key(KeyCode::Esc);
    h.advance(Duration::from_secs(5));
    assert!(!h.app.conns.is_connected(profile));
    assert!(h.connecting().is_none(), "closing the dialog connects nothing");
    assert_eq!(monitor(&h).state, MonState::Closed);
}

#[test]
fn a_paste_goes_into_the_sessions_filter() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    h.keys("/");
    h.event(ratatui::crossterm::event::Event::Paste("alice\n".into()));
    assert_eq!(monitor(&h).list.filter.text(), "alice");
    let m = monitor(&h);
    let list = &m.snapshot.as_ref().unwrap().sessions.as_ref().unwrap().rows;
    assert_eq!(m.list.order.iter().map(|&i| list[i].key.id).collect::<Vec<_>>(), [4211]);
}

#[test]
fn reads_that_keep_failing_slow_down_through_reconnects() {
    let (mut h, mut request) = monitor_open();
    for round in 0..3 {
        let timeout = DbError::NoAnswer(Duration::from_secs(5));
        h.monitor_db(DbEvent::MonitorSnapshot { request, result: Err(timeout.clone()) });
        h.monitor_db(DbEvent::Lost { error: timeout });
        h.advance(Duration::from_secs(120));
        h.monitor_db(DbEvent::Connected);
        h.monitor_db(DbEvent::Monitor(Ok(pg_format())));
        assert!(monitor(&h).failures >= 2 * (round + 1), "round {round}: {}", monitor(&h).failures);
        let t = ticks(&mut h);
        assert_eq!(t.len(), 1, "round {round}");
        request = t[0].0;
    }
    assert!(monitor(&h).delay() > Duration::from_secs(2), "slowed down: {:?}", monitor(&h).delay());
}

#[test]
fn reopening_a_closed_monitor_tab_goes_to_the_open_one() {
    let (mut h, _) = monitor_open();
    h.app.dispatch(datarig_tui::app::action::Action::CloseTab);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    h.command("monitor");
    let monitors = |h: &Harness| h.app.tabs.iter().filter(|t| t.kind == TabKind::Monitor).count();
    assert_eq!(monitors(&h), 1);
    h.keys(" tu");
    assert_eq!(monitors(&h), 1, "one Monitor tab per profile");
    assert_eq!(h.app.tab().kind, TabKind::Monitor);
}

/// The start of session 4211's statement, as the list holds it.
const START: &str = "UPDATE orders SET status = 'paid' WHERE id = 7";

/// Session 4211's whole statement.
const WHOLE: &str = "UPDATE orders SET status = 'paid' WHERE id = 7 AND note = 'the rest of the statement'";

/// What only the whole statement has, on a line of its own however it is laid out.
const REST: &str = "note = 'the rest of the statement'";

/// A read at `secs` of the usual sessions, the list holding only the start of session 4211's
/// statement, which started at `started` (server time).
fn long_snapshot(secs: f64, started: Timestamp, server: bool) -> Snapshot {
    let mut s = snapshot(secs, 1000 + secs as u64, rows(), server);
    let r = s.sessions.as_mut().unwrap().rows.iter_mut().find(|r| r.key.id == 4211).unwrap();
    r.query = Some(QueryText { text: START.into(), shortened: true, maybe_cut_by_server: false });
    r.query_started = Some(started);
    s
}

/// The reads of a whole statement that went out since the last look.
fn text_asks(h: &mut Harness) -> Vec<(u64, TextAsk)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::MonitorQueryText { request, ask } => Some((request, ask)),
            _ => None,
        })
        .collect()
}

/// The Sessions view with session 4211 (its statement shortened) selected, details closed.
fn long_selected() -> Harness {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(long_snapshot(0.0, at(-190.0), true)) });
    h.keys("2j");
    assert_eq!(monitor(&h).selected_row().unwrap().key.id, 4211);
    h
}

/// [`long_selected`] with the details open: the read of the whole statement sent.
fn long_details() -> (Harness, u64) {
    let mut h = long_selected();
    h.key(KeyCode::Enter);
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    (h, asks[0].0)
}

fn text_answer(h: &mut Harness, request: u64, result: Result<FullText, DbError>) {
    h.monitor_db(DbEvent::MonitorQueryText { request, result });
}

fn whole(proof: Proof) -> FullText {
    FullText::Text { text: QueryText { text: WHOLE.into(), shortened: false, maybe_cut_by_server: false }, proof }
}

fn transient(h: &Harness) -> Option<datarig_core::i18n::Msg> {
    h.app.transient.as_ref().map(|(n, _)| n.msg.clone())
}

#[test]
fn opening_the_details_of_a_shortened_statement_reads_its_whole_text_once() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(long_snapshot(0.0, at(-190.0), true)) });
    // The first row's statement is whole in the list: nothing to read.
    h.keys("2");
    assert_eq!(monitor(&h).selected_row().unwrap().key.id, 3907);
    h.key(KeyCode::Enter);
    assert!(monitor(&h).list.details);
    assert!(text_asks(&mut h).is_empty(), "the list holds all of it");
    // The selection moves to the shortened one with the details open: read once.
    h.keys("j");
    assert!(text_asks(&mut h).is_empty(), "j scrolls the details");
    h.key(KeyCode::Esc);
    h.keys("j");
    h.key(KeyCode::Enter);
    let asks = text_asks(&mut h);
    let key = SessionKey { id: 4211, started: Some(at(-3600.0)) };
    assert_eq!(asks.len(), 1, "{asks:?}");
    let ask = TextAsk { session: key, query_started: Some(at(-190.0)), start: START.into(), seen: at(0.0) };
    assert_eq!(asks[0].1, ask);
    // Not again: not while it is out, not on the next reads, not when the details open again.
    assert!(ticks(&mut h).is_empty());
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1);
    assert!(text_asks(&mut h).is_empty());
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(long_snapshot(2.0, at(-190.0), false)) });
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    text_answer(&mut h, asks[0].0, Ok(whole(Proof::Same)));
    h.advance(Duration::from_secs(2));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    assert!(text_asks(&mut h).is_empty(), "asked once for this statement");
    // The tick and the text are numbered apart.
    assert_ne!(asks[0].0, t[0].0);
}

#[test]
fn a_paused_tab_reads_the_whole_statement_the_user_asked_for() {
    let mut h = long_selected();
    h.keys("p");
    h.advance(Duration::from_secs(10));
    assert!(ticks(&mut h).is_empty());
    h.key(KeyCode::Enter);
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert!(ticks(&mut h).is_empty(), "no tick: still paused");
}

#[test]
fn the_whole_statement_is_shown_copied_and_opened() {
    let (mut h, request) = long_details();
    let screen = h.screen(120, 40);
    assert!(screen.contains("Reading the whole statement…"), "{screen}");
    assert!(screen.contains(START) && !screen.contains(REST), "{screen}");
    text_answer(&mut h, request, Ok(whole(Proof::Same)));
    let screen = h.screen(120, 40);
    assert!(screen.contains(REST), "{screen}");
    assert!(!screen.contains("Reading the whole statement"), "{screen}");
    assert!(!screen.contains("The list holds the first"), "{screen}");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some(WHOLE));
    h.keys("o");
    assert_eq!(h.app.tab().kind, TabKind::Console);
    assert_eq!(h.app.tab().editor.text(), WHOLE);
    assert_eq!(transient(&h), Some(datarig_core::i18n::Msg::MonitorOpened { database: "shop".into() }));
}

#[test]
fn a_statement_read_by_its_start_to_the_second_says_so() {
    let (mut h, request) = long_details();
    text_answer(&mut h, request, Ok(whole(Proof::Likely)));
    let screen = h.screen(160, 40);
    assert!(screen.contains(REST), "{screen}");
    assert!(screen.contains("Read again by its session and its start (to the second)"), "{screen}");
}

#[test]
fn a_statement_not_read_whole_keeps_its_start_and_says_why() {
    let cases = [
        (Ok(FullText::Gone), "The session has ended: only the start of its statement is shown"),
        (Ok(FullText::Moved), "The session runs another statement now: only the start of the one listed is shown"),
        (
            Ok(FullText::Unknown(Some(Need::Grant("pg_read_all_stats")))),
            "Cannot tell it is the same statement: only its start is shown (needs pg_read_all_stats)",
        ),
        (Ok(FullText::Unknown(None)), "Cannot tell it is the same statement: only its start is shown"),
        (Ok(FullText::NotYet), "The statement had run under 5 seconds when listed: its whole text is read once"),
        (Err(DbError::Server("permission denied".into())), "Could not read the whole statement: "),
    ];
    for (result, note) in cases {
        let (mut h, request) = long_details();
        text_answer(&mut h, request, result);
        let screen = h.screen(160, 40);
        assert!(screen.contains(note), "{note}: {screen}");
        assert!(screen.contains(START) && !screen.contains(REST), "{note}: {screen}");
        assert!(screen.contains("The list holds the first 256 characters of the statement"), "{note}: {screen}");
        assert!(!screen.contains("Reading the whole statement"), "{note}: {screen}");
        let clip = FakeClipboard::attach(&mut h, false, &[]);
        h.keys("y");
        assert_eq!(clip.last().as_deref(), Some(START), "{note}");
        h.keys("o");
        assert_eq!(h.app.tab().editor.text(), START, "{note}");
        assert_eq!(
            transient(&h),
            Some(datarig_core::i18n::Msg::MonitorOpenedCut { database: "shop".into() }),
            "{note}"
        );
    }
}

#[test]
fn an_answer_of_another_read_or_an_older_connection_is_dropped() {
    let (mut h, request) = long_details();
    // Not the read out.
    text_answer(&mut h, request + 5, Ok(whole(Proof::Same)));
    assert!(!h.screen(120, 40).contains(REST));
    // A tick's number is no text's.
    h.monitor_db(DbEvent::MonitorSnapshot { request, result: Ok(long_snapshot(1.0, at(-190.0), false)) });
    assert!(monitor(&h).text.as_ref().unwrap().answer.is_none());
    // The connection is lost: the read is forgotten, asked again on the new connection.
    let old = monitor(&h).generation;
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    assert!(monitor(&h).text.is_none());
    h.advance(Duration::from_secs(3));
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(pg_format())));
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    let id = h.app.tab().id;
    for (generation, request) in [(old, asks[0].0), (monitor(&h).generation, request)] {
        h.app.on_app_event(datarig_tui::app::AppEvent::Db {
            target: datarig_tui::app::EventTarget::Monitor(id),
            generation,
            ev: DbEvent::MonitorQueryText { request, result: Ok(whole(Proof::Same)) },
        });
        assert!(!h.screen(120, 40).contains(REST), "generation {generation}, request {request}");
    }
    text_answer(&mut h, asks[0].0, Ok(whole(Proof::Same)));
    assert!(h.screen(120, 40).contains(REST));
}

#[test]
fn the_whole_text_is_not_shown_once_the_session_runs_another_statement() {
    let (mut h, request) = long_details();
    text_answer(&mut h, request, Ok(whole(Proof::Same)));
    assert!(h.screen(120, 40).contains(REST));
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    // The same session, a statement with the same start that started later.
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(long_snapshot(2.0, at(1.0), false)) });
    let screen = h.screen(120, 40);
    assert!(!screen.contains(REST), "{screen}");
    assert!(screen.contains(START), "{screen}");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some(START));
    // That statement's own text is read.
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0].1.query_started, Some(at(1.0)));
}

#[test]
fn one_read_of_a_whole_statement_at_a_time() {
    let (mut h, request) = long_details();
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    // Another statement of the session listed while the first read has not answered: not asked
    // for yet, so reads never pile up ahead of the ticks.
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(long_snapshot(2.0, at(1.0), false)) });
    assert_eq!(text_asks(&mut h), []);
    // The first read answers: the statement selected now is asked for, once.
    text_answer(&mut h, request, Ok(FullText::Moved));
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0].1.query_started, Some(at(1.0)));
    assert!(!h.screen(120, 40).contains(REST));
}

#[test]
fn screens_of_the_whole_statement_80x24_and_120x40() {
    let (mut h, request) = long_details();
    text_answer(&mut h, request, Ok(whole(Proof::Likely)));
    insta::assert_snapshot!("monitor_details_whole_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_details_whole_80x24", h.screen(80, 24));
}

fn mysql_format() -> MonitorFormat {
    use datarig_core::monitor::format::{MySqlMonitor, Readable};
    MonitorFormat::MySql(MySqlMonitor {
        version: 80411,
        performance_schema: true,
        user: "root".into(),
        process: Readable::Yes,
        innodb_trx: Readable::Yes,
        metadata_locks: Readable::Yes,
        statements: Readable::Yes,
        flavour: Flavour::Standard,
    })
}

/// A MySQL monitor (whole-second starts) with session 4211 selected and its details open, the
/// list read at `secs` with the statement started at `started`: the read of its whole text sent.
fn mysql_details(secs: f64, started: Timestamp) -> (Harness, u64) {
    let mut h = Harness::connected(Lang::En);
    h.driver.monitor.store(true, Ordering::SeqCst);
    h.command("monitor");
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(mysql_format())));
    let t = ticks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(long_snapshot(secs, started, true)) });
    h.keys("2j");
    h.key(KeyCode::Enter);
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    (h, asks[0].0)
}

/// The next tick, its list read at `secs` with session 4211's statement started at `started`.
fn next_tick(h: &mut Harness, secs: f64, started: Timestamp) {
    h.advance(Duration::from_secs(2));
    let t = ticks(h);
    assert_eq!(t.len(), 1);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(long_snapshot(secs, started, false)) });
}

#[test]
fn a_mysql_start_that_moves_within_a_second_keeps_the_whole_text_and_reads_it_once() {
    let (mut h, request) = mysql_details(0.0, at(-190.0));
    text_answer(&mut h, request, Ok(whole(Proof::Likely)));
    assert!(h.screen(160, 40).contains(REST));
    // The start worked out from whole seconds moves a little from one read to the next.
    for (n, started) in [at(-189.2), at(-190.7), at(-189.9)].into_iter().enumerate() {
        next_tick(&mut h, 2.0 * (n + 1) as f64, started);
        assert!(text_asks(&mut h).is_empty(), "read {n}: not asked again");
        assert!(h.screen(160, 40).contains(REST), "read {n}: still shown");
    }
    // Two seconds or more apart: another statement.
    next_tick(&mut h, 8.0, at(-187.0));
    assert!(!h.screen(160, 40).contains(REST));
    assert_eq!(text_asks(&mut h).len(), 1);
}

#[test]
fn a_statement_too_young_to_tell_apart_is_read_again_once_it_has_run_longer() {
    // Listed after 2 s.
    let (mut h, request) = mysql_details(0.0, at(-2.0));
    text_answer(&mut h, request, Ok(FullText::NotYet));
    let screen = h.screen(160, 40);
    assert!(screen.contains("The statement had run under 5 seconds when listed"), "{screen}");
    assert!(screen.contains(START) && !screen.contains(REST), "{screen}");
    // Listed after 4 s: still too young, not asked again.
    next_tick(&mut h, 2.0, at(-2.0));
    assert!(text_asks(&mut h).is_empty());
    // Listed after 6 s: asked again, once.
    next_tick(&mut h, 4.0, at(-2.0));
    let asks = text_asks(&mut h);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0].1.seen, at(4.0));
    next_tick(&mut h, 6.0, at(-2.0));
    assert!(text_asks(&mut h).is_empty(), "once");
    text_answer(&mut h, asks[0].0, Ok(whole(Proof::Likely)));
    assert!(h.screen(160, 40).contains(REST));
}

#[test]
fn a_statement_the_server_may_have_cut_says_so_and_opens_with_the_cut_note() {
    let (mut h, request) = mysql_details(0.0, at(-190.0));
    let cut = FullText::Text {
        text: QueryText { text: WHOLE.into(), shortened: false, maybe_cut_by_server: true },
        proof: Proof::Likely,
    };
    text_answer(&mut h, request, Ok(cut));
    let screen = h.screen(160, 40);
    assert!(screen.contains(REST), "{screen}");
    assert!(screen.contains("The server may have cut this statement where it keeps it"), "{screen}");
    h.keys("o");
    assert_eq!(h.app.tab().editor.text(), WHOLE);
    assert_eq!(transient(&h), Some(datarig_core::i18n::Msg::MonitorOpenedCut { database: "shop".into() }));
}

// Locks.

use datarig_core::monitor::Blocker;
use datarig_core::monitor::locks::{Blame, Holder, LockHolder, LockList, LockObject, LockWait, Prepared};

/// Whether each monitor read that went out since the last look asked for the locks.
fn lock_asks(h: &mut Harness) -> Vec<(u64, bool)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::MonitorTick { request, want } => Some((request, want.locks)),
            _ => None,
        })
        .collect()
}

/// A chain: bob (3907) holds a row of `orders` in an open transaction; alice's update (4211)
/// waits for it; the ALTER (4302) waits for both; a read (4310) waits behind the ALTER; 4500
/// waits for a prepared transaction.
fn lock_rows() -> Vec<SessionRow> {
    let mut rows = rows();
    rows.retain(|r| r.key.id != 4302);
    let wait = |r: &mut SessionRow, name: &str, by: Vec<Blocker>| {
        r.wait = Some(Wait { class: WaitClass::Lock, name: name.into() });
        r.blocked_by = Some(by);
    };
    let alice = rows.iter_mut().find(|r| r.key.id == 4211).unwrap();
    wait(alice, "transactionid", vec![Blocker::Session(3907)]);
    let mut alter = session(4302, "batch", SessionState::Active, 122.0, "ALTER TABLE orders ADD COLUMN note text");
    wait(&mut alter, "relation", vec![Blocker::Session(4211), Blocker::Session(3907)]);
    let mut read = session(4310, "web", SessionState::Active, 118.0, "SELECT * FROM orders WHERE id = 9");
    wait(&mut read, "relation", vec![Blocker::Session(4302)]);
    let mut invoice = session(4500, "billing", SessionState::Active, 30.0, "INSERT INTO invoices VALUES (1)");
    wait(&mut invoice, "relation", vec![Blocker::PreparedTransaction]);
    rows.extend([alter, read, invoice]);
    rows
}

fn lock_list(secs: f64) -> LockList {
    let key = |id: i64| SessionKey { id, started: Some(at(-3600.0)) };
    let holds = |id: i64, mode: &str| LockHolder {
        holder: Holder::Session(id),
        modes: vec![mode.into()],
        waiting: false,
        blame: Blame::Holds,
        explicit: false,
    };
    let orders = LockObject { kind: "relation".into(), name: Some("orders".into()), ..Default::default() };
    let wait = |id: i64, object: &LockObject, mode: &str, since: f64, holders: Vec<LockHolder>| LockWait {
        waiter: key(id),
        object: object.clone(),
        mode: mode.into(),
        since: Some(at(secs - since)),
        query_started: None,
        whole_seconds: false,
        holders,
    };
    let xid = LockObject { kind: "transactionid".into(), detail: Some("736".into()), ..Default::default() };
    let invoices = LockObject { kind: "relation".into(), name: Some("invoices".into()), ..Default::default() };
    let prepared = LockHolder {
        holder: Holder::Prepared(Some(Prepared {
            gid: "batch-17".into(),
            owner: Some("billing".into()),
            database: Some("shop".into()),
            since: Some(at(secs - 900.0)),
        })),
        modes: vec!["ShareLock".into()],
        waiting: false,
        blame: Blame::Holds,
        explicit: false,
    };
    LockList {
        time: at(secs),
        waits: vec![
            wait(4211, &xid, "ShareLock", 190.0, vec![holds(3907, "ExclusiveLock")]),
            wait(
                4302,
                &orders,
                "AccessExclusiveLock",
                122.0,
                vec![holds(3907, "RowExclusiveLock"), holds(4211, "RowExclusiveLock")],
            ),
            wait(
                4310,
                &orders,
                "AccessShareLock",
                118.0,
                vec![holds(3907, "RowExclusiveLock"), holds(4211, "RowExclusiveLock")],
            ),
            wait(4500, &invoices, "RowExclusiveLock", 30.0, vec![prepared]),
        ],
        more: 0,
        blocks: false,
    }
}

fn lock_snapshot(secs: f64, locks: bool, server: bool) -> Snapshot {
    let mut s = snapshot(secs, 1000, lock_rows(), server);
    s.locks = locks.then(|| lock_list(secs));
    s
}

/// The Locks view shown: a read at once that asks for the locks, then not again before five
/// seconds have passed (the interval is two); the other views never ask.
#[test]
fn the_locks_view_reads_the_lock_table_at_most_every_five_seconds() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, lock_rows(), true)) });
    h.advance(Duration::from_secs(2));
    let t = lock_asks(&mut h);
    assert_eq!(t.iter().map(|t| t.1).collect::<Vec<_>>(), [false], "the Overview asks for no locks");
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(2.0, 1000, lock_rows(), false)) });
    h.keys("3");
    assert_eq!(monitor(&h).view, MonitorView::Locks);
    let t = lock_asks(&mut h);
    assert_eq!(t.iter().map(|t| t.1).collect::<Vec<_>>(), [true], "shown, it reads at once with the locks");
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(lock_snapshot(2.0, true, false)) });
    let mut asked = Vec::new();
    for n in 1..=3 {
        h.advance(Duration::from_secs(2));
        let t = lock_asks(&mut h);
        assert_eq!(t.len(), 1);
        asked.push(t[0].1);
        h.monitor_db(DbEvent::MonitorSnapshot {
            request: t[0].0,
            result: Ok(lock_snapshot(2.0 + 2.0 * n as f64, t[0].1, false)),
        });
    }
    assert_eq!(asked, [false, false, true], "2 s and 4 s after: not yet; 6 s after: again");
    // The lock read stays between reads of it.
    assert!(monitor(&h).locks.is_some());
    h.keys("2");
    h.advance(Duration::from_secs(10));
    let t = lock_asks(&mut h);
    assert!(t.iter().all(|t| !t.1), "the Sessions view asks for no locks: {t:?}");
}

/// The tree: root blockers first, each session once under the one it waits for, what each
/// waits for and holds; `b` goes to the root blocker; `Enter` shows a session in full.
#[test]
fn the_locks_view_shows_the_blocking_tree_and_goes_to_the_root_blocker() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, lock_rows(), true)) });
    h.keys("3");
    let t = lock_asks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(lock_snapshot(2.0, true, false)) });
    let nodes: Vec<Holder> = monitor(&h).tree.lines.iter().map(|l| l.node.clone()).collect();
    // The ALTER waits for bob too: it is under him, nearest the root, with more behind it.
    assert_eq!(
        nodes[..4],
        [Holder::Session(3907), Holder::Session(4302), Holder::Session(4310), Holder::Session(4211)]
    );
    assert!(matches!(&nodes[4], Holder::Prepared(Some(p)) if p.gid == "batch-17"), "{nodes:?}");
    assert_eq!(nodes[5], Holder::Session(4500));
    let screen = h.screen(160, 40);
    assert!(screen.contains("3 Locks (4)"), "{screen}");
    assert!(screen.contains("4 waiting for a lock"), "{screen}");
    assert!(screen.contains("▸ 3907 bob · idle in transaction 12m"), "{screen}");
    assert!(screen.contains("holds ExclusiveLock on transactionid 736"), "{screen}");
    assert!(screen.contains("└ 4211 alice · waits 3m10s for ShareLock on transactionid 736"), "{screen}");
    assert!(
        screen.contains("├ 4302 batch · waits 2m02s for AccessExclusiveLock on relation orders · also waits for 4211"),
        "{screen}"
    );
    assert!(screen.contains("│ └ 4310 web · waits 1m58s for AccessShareLock on relation orders"), "{screen}");
    assert!(
        screen.contains("▸ prepared transaction batch-17 · owner billing, database shop, prepared 15m00s ago"),
        "{screen}"
    );
    // From the read behind the ALTER, `b` goes to the root.
    h.keys("jj");
    assert_eq!(monitor(&h).lock_view.selected, Some(Holder::Session(4310)));
    h.keys("b");
    assert_eq!(monitor(&h).lock_view.cursor, 0);
    // The selection stays on its session across reads.
    h.keys("jjj");
    h.advance(Duration::from_secs(2));
    let t = lock_asks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(lock_snapshot(4.0, false, false)) });
    assert_eq!(monitor(&h).lock_view.selected, Some(Holder::Session(4211)));
    h.key(KeyCode::Enter);
    assert_eq!(monitor(&h).view, MonitorView::Sessions);
    assert!(monitor(&h).list.details);
    assert_eq!(monitor(&h).selected_row().map(|r| r.key.id), Some(4211));
}

/// A click selects a line; the wheel moves; `y` and `o` take the selected line's statement.
#[test]
fn the_locks_view_takes_clicks_and_its_lines_statement() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, lock_rows(), true)) });
    h.keys("3");
    let t = lock_asks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(lock_snapshot(2.0, true, false)) });
    let screen = h.screen(120, 40);
    let line = screen.lines().position(|l| l.contains("4211 alice")).expect("alice's line");
    let text = screen.lines().nth(line).unwrap();
    let x = text[..text.find("4211 alice").unwrap()].chars().count() as u16;
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, line as u16);
    assert_eq!(monitor(&h).lock_view.selected, Some(Holder::Session(4211)));
    h.mouse(MouseEventKind::ScrollUp, x, line as u16);
    assert_eq!(monitor(&h).lock_view.selected, Some(Holder::Session(3907)), "up to the first line");
    h.keys("j");
    h.keys("o");
    assert_eq!(h.app.tab().kind, TabKind::Console);
    assert_eq!(h.app.tab().editor.text(), "ALTER TABLE orders ADD COLUMN note text");
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::Execute { .. })), "the statement is never run");
}

#[test]
fn the_locks_view_says_when_nothing_waits_and_what_the_reads_left_out() {
    let (mut h, first) = monitor_open();
    let quiet = || {
        let mut rows = rows();
        rows.iter_mut().for_each(|r| r.wait = None);
        rows
    };
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, quiet(), true)) });
    h.keys("3");
    let t = lock_asks(&mut h);
    let mut s = snapshot(2.0, 1000, quiet(), false);
    s.locks = Some(LockList { time: at(2.0), waits: Vec::new(), more: 0, blocks: true });
    s.hidden.push(Hidden { part: Part::RowLockWaits, need: Need::Grant("SELECT on performance_schema") });
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(s) });
    let note = "Who blocks a row lock wait is not shown (needs SELECT on performance_schema)";
    let screen = h.screen(160, 40);
    assert!(screen.contains("No session waits for a lock"), "{screen}");
    assert!(screen.contains(note), "{screen}");
    // The next read does not read the locks: what the last read of them could not see stays said.
    h.advance(Duration::from_secs(2));
    let t = lock_asks(&mut h);
    assert!(!t[0].1);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(4.0, 1000, quiet(), false)) });
    assert!(h.screen(160, 40).contains(note));
    // A session waiting whose blocker could not be read is shown waiting.
    h.advance(Duration::from_secs(2));
    let t = lock_asks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(6.0, 1000, rows(), false)) });
    let screen = h.screen(160, 40);
    assert!(screen.contains("4211 alice · waits for a lock (transactionid) · "), "{screen}");
}

/// Waits the account may not see are not said to be none: without `pg_read_all_stats` (or,
/// on MySQL, `PROCESS`) other accounts' sessions may wait unseen.
#[test]
fn the_locks_view_does_not_say_no_session_waits_when_some_cannot_be_seen() {
    let (mut h, first) = monitor_open();
    let quiet = || {
        let mut rows = rows();
        rows.iter_mut().for_each(|r| r.wait = None);
        rows
    };
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, quiet(), true)) });
    h.keys("3");
    let t = lock_asks(&mut h);
    let mut s = snapshot(2.0, 1000, quiet(), false);
    s.hidden.push(Hidden { part: Part::OtherSessionDetails, need: Need::Grant("pg_read_all_stats") });
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(s) });
    let screen = h.screen(160, 40);
    assert!(!screen.contains("No session waits for a lock"), "{screen}");
    assert!(screen.contains("None of the sessions this account may see waits for a lock"), "{screen}");
}

#[test]
fn locks_screens_80x24_and_120x40() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, lock_rows(), true)) });
    h.keys("3");
    let t = lock_asks(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(lock_snapshot(2.0, true, false)) });
    insta::assert_snapshot!("monitor_locks_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_locks_80x24", h.screen(80, 24));
}

// The Replication view and the server's facts in full.

use datarig_core::monitor::replication::{ChannelError, Member, Replica, Running, Side, Slot, Upstream};
use datarig_core::monitor::{Lsn, Replication};

/// The reads that went out since the last look: the ticks' and the replication reads' request
/// ids.
fn reads(h: &mut Harness) -> (Vec<u64>, Vec<u64>) {
    let mut ticks = Vec::new();
    let mut replication = Vec::new();
    for c in h.sent() {
        match c {
            DbCommand::MonitorTick { request, .. } => ticks.push(request),
            DbCommand::MonitorReplication { request, limit } => {
                assert!(limit <= Duration::from_secs(5), "{limit:?}");
                replication.push(request);
            }
            _ => {}
        }
    }
    (ticks, replication)
}

fn upstream(source: &str) -> Upstream {
    Upstream {
        channel: None,
        source: Some(source.into()),
        slot: None,
        receiver: Running::Yes,
        receiver_state: None,
        applier: Running::Yes,
        applier_state: None,
        received: None,
        applied: None,
        received_set: None,
        executed_set: None,
        gtids_only: false,
        gtids_left: None,
        source_read: None,
        source_applied: None,
        applier_behind: None,
        delay: None,
        last_message: None,
        last_received: None,
        last_applied: None,
        errors: Vec::new(),
        visible: Visibility::Full,
    }
}

fn replica(name: &str) -> Replica {
    Replica {
        id: Some(87),
        name: Some(name.into()),
        client: Some("10.0.0.21".into()),
        state: Some("streaming".into()),
        sync: Some("async".into()),
        sent: Some(Lsn(0x3C8_4EC8)),
        written: Some(Lsn(0x3C8_4EC8)),
        flushed: Some(Lsn(0x3C8_4EC8)),
        replayed: Some(Lsn(0x3C8_4EC8)),
        write_lag: Some(1_096),
        flush_lag: Some(1_770),
        replay_lag: Some(1_834),
        reply: Some(at(-1.0)),
        user: None,
        visible: Visibility::Full,
    }
}

fn slot(name: &str, active: bool, kept: u64) -> Slot {
    Slot {
        name: name.into(),
        kind: "physical".into(),
        plugin: None,
        database: None,
        temporary: false,
        active,
        active_pid: active.then_some(87),
        restart: Some(Lsn(0x3C8_4EC8 - kept)),
        kept: Some(kept),
        wal_status: Some("reserved".into()),
        safe_wal_size: None,
    }
}

/// A PostgreSQL primary as the dev pair showed it: one standby streaming, its slot, a slot whose
/// consumer is away (keeping log) and a logical one.
fn pg_primary() -> Replication {
    let mut behind = replica("reporting");
    behind.client = Some("10.0.0.22".into());
    behind.replayed = Some(Lsn(0x3C8_4EC8 - 3 * 1024 * 1024));
    behind.write_lag = None;
    behind.flush_lag = None;
    behind.replay_lag = Some(4_200_000);
    let mut logical = slot("zz_logical", false, 12_582_912);
    logical.kind = "logical".into();
    logical.plugin = Some("pgoutput".into());
    logical.database = Some("shop".into());
    Replication {
        time: at(0.0),
        in_recovery: Some(false),
        position: Some(Lsn(0x3C8_4EC8)),
        upstream: Some(Vec::new()),
        replicas: Some(vec![replica("walreceiver"), behind]),
        slots: Some(vec![slot("standby1", true, 0), slot("zz_idle_slot", false, 13_107_200), logical]),
        subscriptions: Some(Vec::new()),
        members: Vec::new(),
        hidden: Vec::new(),
    }
}

/// A PostgreSQL standby whose replay is paused while its primary writes.
fn pg_standby() -> Replication {
    let mut u = upstream("pg-primary:5432");
    u.slot = Some("standby1".into());
    u.applier = Running::Paused;
    u.received = Some(Lsn(0x3C8_4EC8));
    u.applied = Some(Lsn(0x3A0_0000));
    u.last_message = Some(at(-1.0));
    u.last_applied = Some(at(-190.0));
    Replication {
        time: at(0.0),
        in_recovery: Some(true),
        position: Some(Lsn(0x3C8_4EC8)),
        upstream: Some(vec![u]),
        replicas: Some(Vec::new()),
        slots: Some(Vec::new()),
        subscriptions: Some(Vec::new()),
        members: Vec::new(),
        hidden: Vec::new(),
    }
}

/// A MySQL replica whose applier stopped on a duplicate key while its receiver runs.
fn mysql_replica() -> Replication {
    let mut c = upstream("my-source:3306");
    c.receiver_state = Some("Waiting for source to send event".into());
    c.applier = Running::No;
    c.gtids_only = true;
    c.gtids_left = Some(2);
    c.source_read = Some(datarig_core::monitor::replication::LogPos { file: "bin.000003".into(), pos: 3256 });
    c.source_applied = Some(datarig_core::monitor::replication::LogPos { file: "bin.000003".into(), pos: 2730 });
    c.received_set = Some("ced11181-db1f-11f1-9e15-967323c11f94:1-12".into());
    c.executed_set =
        Some("ccf08818-db1f-11f1-bcb9-6aa192f2d4c5:1-5,\nced11181-db1f-11f1-9e15-967323c11f94:1-10".into());
    c.last_message = Some(at(-12.0));
    c.last_received = Some(at(-40.0));
    c.last_applied = Some(at(-41.0));
    c.errors = vec![ChannelError {
        side: Side::Applier,
        code: 1062,
        message: "Coordinator stopped because there were error(s) in the worker(s).".into(),
        at: Some("261108 03:10:42".into()),
    }];
    Replication {
        time: at(0.0),
        in_recovery: None,
        position: None,
        upstream: Some(vec![c]),
        replicas: None,
        slots: None,
        subscriptions: None,
        members: vec![Member {
            address: "my-replica:3306".into(),
            state: "ONLINE".into(),
            role: Some("SECONDARY".into()),
            this: true,
        }],
        hidden: vec![Hidden { part: Part::Replicas, need: Need::Grant("PROCESS") }],
    }
}

/// The Monitor tab on the Replication view, its first reads answered.
fn on_replication(format: MonitorFormat, read: Replication) -> Harness {
    let mysql = matches!(format, MonitorFormat::MySql(_));
    let mut h = Harness::connected(Lang::En);
    h.driver.monitor.store(true, Ordering::SeqCst);
    h.command("monitor");
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(format)));
    let (t, _) = reads(&mut h);
    let mut first = snapshot(0.0, 1000, rows(), true);
    if mysql {
        first.server.as_mut().unwrap().version = "8.4.11 MySQL Community Server - GPL".into();
    }
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0], result: Ok(first) });
    h.keys("4");
    let (t, r) = reads(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0], result: Ok(snapshot(0.5, 1100, rows(), false)) });
    h.monitor_db(DbEvent::MonitorReplication { request: r[0], result: Ok(read) });
    h
}

#[test]
fn the_replication_view_reads_when_shown_then_every_ten_seconds_and_only_while_shown() {
    let (mut h, first) = monitor_open();
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(snapshot(0.0, 1000, rows(), true)) });
    assert!(reads(&mut h).1.is_empty(), "nothing of replication while another view is shown");
    // Shown: read at once, with a tick.
    h.keys("4");
    assert_eq!(monitor(&h).view, MonitorView::Replication);
    let (t, r) = reads(&mut h);
    assert_eq!((t.len(), r.len()), (1, 1));
    h.monitor_db(DbEvent::MonitorReplication { request: r[0], result: Ok(pg_primary()) });
    let mut answer = t[0];
    // The ticks go on every 2 s; replication only every 10 s.
    let mut replication_reads = 0;
    for n in 1..=10 {
        h.monitor_db(DbEvent::MonitorSnapshot {
            request: answer,
            result: Ok(snapshot(n as f64 * 2.0, 1000, rows(), false)),
        });
        h.advance(Duration::from_millis(2001));
        let (t, r) = reads(&mut h);
        assert_eq!(t.len(), 1);
        answer = t[0];
        for request in r {
            replication_reads += 1;
            h.monitor_db(DbEvent::MonitorReplication { request, result: Ok(pg_primary()) });
        }
    }
    assert_eq!(replication_reads, 2, "20 s shown: two more reads");
    h.monitor_db(DbEvent::MonitorSnapshot { request: answer, result: Ok(snapshot(30.0, 1000, rows(), false)) });
    // Another view: none.
    h.keys("1");
    for _ in 0..10 {
        h.advance(Duration::from_millis(2001));
        let (t, r) = reads(&mut h);
        assert!(r.is_empty());
        if let Some(request) = t.first() {
            h.monitor_db(DbEvent::MonitorSnapshot {
                request: *request,
                result: Ok(snapshot(40.0, 1000, rows(), false)),
            });
        }
    }
    // Back on it: at once (it was not shown), and `r` reads it again at once.
    h.keys("4");
    h.advance(Duration::from_millis(2001));
    let (t, r) = reads(&mut h);
    assert_eq!(r.len(), 1, "{t:?}");
    h.monitor_db(DbEvent::MonitorReplication { request: r[0], result: Ok(pg_primary()) });
    for request in t {
        h.monitor_db(DbEvent::MonitorSnapshot { request, result: Ok(snapshot(60.0, 1000, rows(), false)) });
    }
    h.keys("r");
    assert_eq!(reads(&mut h).1.len(), 1);
}

#[test]
fn a_failed_replication_read_keeps_the_last_one_and_says_why_and_late_answers_are_dropped() {
    let mut h = on_replication(pg_format(), pg_primary());
    assert!(monitor(&h).replication.is_some());
    h.keys("r");
    let (t, r) = reads(&mut h);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0], result: Ok(snapshot(3.0, 1000, rows(), true)) });
    // An answer to a read that is not the one out is dropped.
    h.monitor_db(DbEvent::MonitorReplication { request: r[0] + 9, result: Ok(pg_standby()) });
    assert_eq!(monitor(&h).replication.as_ref().unwrap().in_recovery, Some(false));
    h.monitor_db(DbEvent::MonitorReplication {
        request: r[0],
        result: Err(DbError::Server("function aurora_replica_status() does not exist".into())),
    });
    assert_eq!(monitor(&h).replication.as_ref().unwrap().in_recovery, Some(false), "the last good read stays");
    let screen = h.screen(120, 40);
    assert!(screen.contains("Could not read replication: function aurora_replica_status() does not exist"), "{screen}");
    assert!(screen.contains("Role: primary"), "{screen}");
    // The other views read on: replication's failure is its own.
    assert!(monitor(&h).error.is_none());
}

#[test]
fn replication_screens_80x24_and_120x40() {
    let mut h = on_replication(pg_format(), pg_primary());
    insta::assert_snapshot!("monitor_replication_primary_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_replication_primary_80x24", h.screen(80, 24));
    let mut h = on_replication(pg_format(), pg_standby());
    insta::assert_snapshot!("monitor_replication_standby_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_replication_standby_80x24", h.screen(80, 24));
    let mut h = on_replication(mysql_format(), mysql_replica());
    insta::assert_snapshot!("monitor_replication_mysql_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_replication_mysql_80x24", h.screen(80, 24));
}

#[test]
fn replication_scrolls_and_says_what_it_cannot_tell() {
    let mut hidden = pg_primary();
    hidden.replicas.as_mut().unwrap()[0] = Replica {
        state: None,
        sync: None,
        replayed: None,
        name: None,
        client: None,
        visible: Visibility::Hidden,
        ..replica("")
    };
    hidden.hidden = vec![Hidden { part: Part::ReplicationDetails, need: Need::Grant("pg_read_all_stats") }];
    let mut h = on_replication(pg_format(), hidden);
    let screen = h.screen(120, 40);
    assert!(screen.contains("Replication streams are listed without their state and positions"), "{screen}");
    assert!(screen.contains("pid 87") && screen.contains("state and positions hidden"), "{screen}");
    assert!(!screen.contains("pid 87 · replayed all"), "hidden is not caught up: {screen}");
    // Scrolled to its end at 80x24 the last slot is on screen, and no further.
    h.screen(80, 24);
    h.keys("G");
    let screen = h.screen(80, 24);
    assert!(screen.contains("zz_logical"), "{screen}");
    let top = monitor(&h).replication_top;
    h.keys("j");
    h.screen(80, 24);
    assert_eq!(monitor(&h).replication_top, top, "kept within the lines");
    h.keys("gg");
    h.screen(80, 24);
    assert_eq!(monitor(&h).replication_top, 0);
}

#[test]
fn behind_by_an_amount_not_known_still_says_behind() {
    let mut read = mysql_replica();
    let c = &mut read.upstream.as_mut().unwrap()[0];
    c.gtids_only = false;
    c.gtids_left = None;
    c.errors.clear();
    c.source_read = Some(datarig_core::monitor::replication::LogPos { file: "bin.000004".into(), pos: 120 });
    let mut h = on_replication(mysql_format(), read);
    let screen = h.screen(120, 40);
    assert!(screen.contains("has not applied all it received (how much is not known)"), "{screen}");
}

#[test]
fn on_aurora_no_binlog_replication_is_not_not_replicating() {
    let mut read = pg_primary();
    read.replicas = Some(Vec::new());
    read.slots = Some(Vec::new());
    let aurora = MonitorFormat::Postgres(PgMonitor {
        flavour: Flavour::AmazonAurora,
        ..match pg_format() {
            MonitorFormat::Postgres(p) => p,
            _ => unreachable!(),
        }
    });
    let mut h = on_replication(aurora, read);
    let screen = h.screen(120, 40);
    assert!(screen.contains("Role: cannot tell"), "{screen}");
    assert!(screen.contains("Amazon Aurora: the cluster's own replicas share its storage"), "{screen}");
}

#[test]
fn a_logical_subscriber_is_not_shown_as_not_replicating() {
    let mut read = pg_primary();
    read.replicas = Some(Vec::new());
    read.slots = Some(Vec::new());
    read.subscriptions = Some(vec![
        datarig_core::monitor::replication::Subscription {
            name: "orders_sub".into(),
            running: true,
            received: Some(Lsn(0x3C8_4EC8)),
            last_message: Some(at(-2.0)),
        },
        datarig_core::monitor::replication::Subscription {
            name: "audit_sub".into(),
            running: false,
            received: None,
            last_message: None,
        },
    ]);
    let mut h = on_replication(pg_format(), read);
    let screen = h.screen(120, 40);
    assert!(screen.contains("Role: logical subscriber"), "{screen}");
    assert!(
        screen.contains("orders_sub · running · received 0/3C84EC8 · last heard from the source 2s ago"),
        "{screen}"
    );
    assert!(screen.contains("audit_sub · stopped"), "{screen}");
}

#[test]
fn the_server_in_full_opens_with_i_over_any_view_and_closes_with_i_or_esc() {
    let (mut h, first) = monitor_open();
    let mut s = snapshot(0.0, 1000, rows(), true);
    s.server.as_mut().unwrap().extra = vec![
        datarig_core::monitor::Extra { key: datarig_core::monitor::ExtraKey::TrackActivities, value: "on".into() },
        datarig_core::monitor::Extra { key: datarig_core::monitor::ExtraKey::TrackCounts, value: "off".into() },
        datarig_core::monitor::Extra { key: datarig_core::monitor::ExtraKey::QuerySize, value: "1kB".into() },
        datarig_core::monitor::Extra { key: datarig_core::monitor::ExtraKey::WalLevel, value: "replica".into() },
    ];
    h.monitor_db(DbEvent::MonitorSnapshot { request: first, result: Ok(s) });
    h.keys("i");
    assert_eq!(monitor(&h).server_info, Some(0));
    insta::assert_snapshot!("monitor_server_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_server_80x24", h.screen(80, 24));
    let screen = h.screen(120, 40);
    assert!(screen.contains("41 of 41") || screen.contains("of 200"), "{screen}");
    assert!(
        screen.contains("track_counts") && screen.contains("off (for this connection; where it is off"),
        "{screen}"
    );
    // A pane longer than its room (many settings at 80x24): G to its end, k up, gg to its top.
    h.keys("r");
    let (t, _) = reads(&mut h);
    let mut long = snapshot(2.0, 1000, rows(), true);
    let setting =
        datarig_core::monitor::Extra { key: datarig_core::monitor::ExtraKey::TrackActivities, value: "on".into() };
    long.server.as_mut().unwrap().extra = vec![setting; 14];
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0], result: Ok(long) });
    h.screen(80, 24);
    h.keys("G");
    h.screen(80, 24);
    let end = monitor(&h).server_info.unwrap();
    assert!(end > 0, "scrolled: {}", h.screen(80, 24));
    assert!(h.screen(80, 24).contains("i or Esc closes"));
    h.keys("k");
    h.screen(80, 24);
    assert_eq!(monitor(&h).server_info, Some(end - 1));
    // A page is the pane's own height, not the list's behind it.
    h.keys("gg");
    h.screen(80, 24);
    h.key(KeyCode::PageDown);
    h.screen(80, 24);
    let page = monitor(&h).server_info.unwrap();
    assert!(page > 3 && page <= end, "{page} of {end}");
    h.keys("gg");
    h.screen(80, 24);
    assert_eq!(monitor(&h).server_info, Some(0));
    h.keys("i");
    assert_eq!(monitor(&h).server_info, None);
    h.keys("2");
    h.keys("i");
    assert!(monitor(&h).server_info.is_some());
    h.key(KeyCode::Esc);
    assert_eq!(monitor(&h).server_info, None);
    assert_eq!(monitor(&h).view, MonitorView::Sessions, "Esc closed the pane only");
    // Over the Locks view the pane takes the scrolling keys first.
    h.keys("3");
    h.keys("i");
    h.screen(80, 24);
    h.keys("G");
    h.screen(80, 24);
    assert!(monitor(&h).server_info.is_some_and(|t| t > 0), "the pane scrolled, not the tree");
}

// ── cancelling and ending sessions ───────────────────────────────────────────────────────

use datarig_core::driver::OwnSession;
use datarig_core::i18n::Label;
use datarig_core::monitor::action::{Denied, NotSent, Stale, Via};
use datarig_core::monitor::{ActionKind, ActionOutcome, ActionTarget, FollowUp};
use datarig_tui::app::overlay::OverlayKind;

/// The Monitor tab of `h` (connected, with a driver that monitors) opened with `format`, its
/// first read answered with `rows`, in the Sessions view.
fn sessions_on(h: Harness, format: MonitorFormat, rows: Vec<SessionRow>) -> Harness {
    sessions_asked(h, format, rows).0
}

/// [`sessions_on`], and the server-session asks sent to the first query session meanwhile.
fn sessions_asked(mut h: Harness, format: MonitorFormat, rows: Vec<SessionRow>) -> (Harness, Vec<u64>) {
    h.driver.monitor.store(true, Ordering::SeqCst);
    h.command("monitor");
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(format)));
    let asks = h.roles().iter().position(|r| *r == SessionRole::Query).map_or(Vec::new(), |q| own_asks(&mut h, q));
    let t = ticks(&mut h);
    assert_eq!(t.len(), 1);
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(0.0, 1000, rows, true)) });
    h.keys("2");
    (h, asks)
}

/// Session `id` selected in the Sessions view.
fn select(h: &mut Harness, id: i64) {
    h.key(KeyCode::Home);
    for _ in 0..8 {
        if monitor(h).selected_row().is_some_and(|r| r.key.id == id) {
            return;
        }
        h.keys("j");
    }
    panic!("session {id} is not listed");
}

/// The usual PostgreSQL sessions in the Sessions view, session `id` selected.
fn pg_selected(id: i64) -> Harness {
    let mut h = sessions_on(Harness::connected(Lang::En), pg_format(), rows());
    select(&mut h, id);
    h
}

/// The actions sent since the last look.
fn actions_sent(h: &mut Harness) -> Vec<(u64, ActionKind, ActionTarget)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::MonitorAction { request, kind, target } => Some((request, kind, target)),
            _ => None,
        })
        .collect()
}

/// The reads that follow actions up, sent since the last look.
fn follow_ups(h: &mut Harness) -> Vec<(u64, ActionTarget)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::MonitorFollowUp { request, target } => Some((request, target)),
            _ => None,
        })
        .collect()
}

/// What the status bar last flashed, in words.
fn said(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| h.app.i18n.msg(&n.msg).to_string()).unwrap_or_default()
}

/// The status bar's question, longest form first.
fn asked(h: &Harness) -> Option<String> {
    h.app.status_ask_line().map(|(forms, _)| forms[0].clone())
}

#[test]
fn c_asks_on_the_status_line_and_only_y_sends_the_cancel_of_that_row() {
    let mut h = pg_selected(4211);
    h.keys("c");
    let q = asked(&h).expect("asked");
    assert!(q.starts_with("Cancel the statement of session 4211 (alice, shop, "), "{q}");
    assert!(q.ends_with("? y/n"), "{q}");
    assert!(actions_sent(&mut h).is_empty(), "nothing before the answer");
    h.keys("y");
    let sent = actions_sent(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let row = monitor(&h).selected_row().unwrap().clone();
    assert_eq!(sent[0].1, ActionKind::Cancel);
    assert_eq!(sent[0].2.key, row.key, "the row's session and its start");
    assert_eq!(sent[0].2.stamp, row.statement, "the row's statement");
    assert!(asked(&h).is_none());
    // Sent is not done: nothing is said to be cancelled before the answer.
    assert!(!said(&h).contains("cancelled"), "{}", said(&h));
    // Any other key sends nothing.
    h.keys("c");
    assert!(asked(&h).is_some());
    h.keys("n");
    assert!(asked(&h).is_none());
    assert_eq!(said(&h), "Nothing sent");
    assert!(actions_sent(&mut h).is_empty());
    h.keys("c");
    h.key(KeyCode::Enter);
    assert!(actions_sent(&mut h).is_empty(), "Enter is no answer");
}

#[test]
fn x_opens_a_dialog_where_enter_keeps_the_session_and_y_ends_it() {
    let mut h = pg_selected(4211);
    h.keys("X");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let screen = h.screen(120, 40);
    for text in ["End the session?", "Session 4211: alice on shop", "Its open transaction is rolled back.", "Keep"] {
        assert!(screen.contains(text), "{text}: {screen}");
    }
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None, "Enter closes it");
    assert!(actions_sent(&mut h).is_empty(), "and keeps the session");
    h.keys("X");
    h.key(KeyCode::Esc);
    assert!(actions_sent(&mut h).is_empty());
    h.keys("X");
    h.keys("y");
    assert_eq!(h.overlay_kind(), None);
    let sent = actions_sent(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].1, ActionKind::Terminate);
    assert_eq!(sent[0].2.key.id, 4211);
}

#[test]
fn a_read_only_environment_refuses_both_and_sends_nothing() {
    let mut cfg = test_db_config();
    cfg.connections[0].policy = Some("prod".into());
    cfg.policies.insert("prod", datarig_core::policy::Policy { read_only: true, ..Default::default() });
    let mut h = sessions_on(Harness::connected_with(&cfg, Lang::En), pg_format(), rows());
    select(&mut h, 4211);
    for key in ["c", "X"] {
        h.keys(key);
        assert_eq!(said(&h), "Read-only environment: the monitor only looks", "{key}");
        assert!(asked(&h).is_none(), "{key}");
        assert_eq!(h.overlay_kind(), None, "{key}");
    }
    assert!(actions_sent(&mut h).is_empty());
}

#[test]
fn the_monitors_own_connection_and_a_session_running_nothing_are_refused() {
    let mut h = pg_selected(4400);
    h.keys("c");
    assert_eq!(said(&h), "This is the monitor's own connection");
    h.keys("X");
    assert_eq!(said(&h), "This is the monitor's own connection");
    select(&mut h, 3907);
    h.keys("c");
    assert_eq!(said(&h), "Session 3907 runs no statement");
    assert!(asked(&h).is_none());
    assert_eq!(h.overlay_kind(), None);
    assert!(actions_sent(&mut h).is_empty());
    // The Overview acts on nothing.
    h.keys("1");
    h.keys("c");
    assert_eq!(said(&h), "Select a session in the Sessions view");
    assert!(actions_sent(&mut h).is_empty());
}

/// The usual sessions as MySQL lists them (statement events), session 4302's statement `q`.
fn mysql_rows(q: &str) -> Vec<SessionRow> {
    let mut rows = rows();
    for r in &mut rows {
        r.thread = Some(r.key.id as u64 + 100);
        r.statement = Some(StatementStamp::MySqlEvent { thread_id: r.key.id as u64 + 100, event_id: 9 });
        r.key.started = None;
        if r.key.id == 4302 {
            r.query = Some(QueryText { text: q.into(), shortened: false, maybe_cut_by_server: false });
        }
    }
    rows
}

#[test]
fn a_mysql_cancel_asks_a_line_for_a_plain_read_and_a_dialog_with_the_warning_otherwise() {
    let mysql = |q: &str| {
        let mut h = sessions_on(Harness::connected(Lang::En), mysql_format(), mysql_rows(q));
        select(&mut h, 4302);
        h.keys("c");
        h
    };
    let mut h = mysql("SELECT count(*) FROM order_items");
    assert!(asked(&h).is_some_and(|q| q.starts_with("Cancel the statement of session 4302")), "{:?}", asked(&h));
    assert_eq!(h.overlay_kind(), None);
    h.keys("y");
    assert_eq!(actions_sent(&mut h).len(), 1);
    let cases = [
        ("UPDATE orders SET status = 'paid' WHERE id = 7", Label::MonitorActWarnHalfDone, "Not a plain read"),
        ("ALTER TABLE orders ADD COLUMN note text", Label::MonitorActWarnMaintenance, "Stopping table maintenance"),
    ];
    for (q, warning, text) in cases {
        let mut h = mysql(q);
        assert!(asked(&h).is_none(), "{q}");
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "{q}");
        let details = &h.app.overlays.confirm().unwrap().details;
        assert!(details.contains(&warning.into()), "{q}: {details:?}");
        let screen = h.screen(120, 40);
        assert!(screen.contains("Cancel the statement?") && screen.contains(text), "{q}: {screen}");
        h.key(KeyCode::Enter);
        assert!(actions_sent(&mut h).is_empty(), "{q}");
    }
}

#[test]
fn a_sent_cancel_is_followed_up_at_1_3_and_10_seconds_even_paused_and_hidden() {
    let mut h = pg_selected(4211);
    let row = monitor(&h).selected_row().unwrap().clone();
    h.keys("c");
    h.keys("y");
    let request = actions_sent(&mut h)[0].0;
    let monitor_tab = h.app.tab().id;
    h.keys("p");
    h.key_mod(KeyCode::PageUp, ratatui::crossterm::event::KeyModifiers::CONTROL);
    assert_ne!(h.app.tab().id, monitor_tab, "hidden");
    let mut words = Vec::new();
    h.monitor_db_to(
        1,
        DbEvent::MonitorActionDone { request, outcome: ActionOutcome::Acknowledged { via: Via::Standard } },
    );
    assert_eq!(said(&h), "Cancel sent to session 4211");
    words.push(said(&h));
    assert!(h.app.monitor_due().is_some(), "a hidden tab still wakes for its follow-ups");
    let still = FollowUp { row: Some(row.clone()), interrupted: None };
    // 1 s, 3 s and 10 s after the answer, each answered as it goes.
    let answered = |h: &mut Harness, result: FollowUp, at: Duration| {
        h.advance(at - Duration::from_millis(100));
        assert!(follow_ups(h).is_empty(), "not before {at:?}");
        h.advance(Duration::from_millis(100));
        let f = follow_ups(h);
        assert_eq!(f.len(), 1, "{at:?}: {f:?}");
        assert_eq!(f[0].1.key, row.key);
        h.monitor_db_to(1, DbEvent::MonitorFollowUp { request: f[0].0, result: Ok(result) });
        said(h)
    };
    words.push(answered(&mut h, still.clone(), Duration::from_secs(1)));
    assert_eq!(words.last().unwrap(), "Session 4211 still runs that statement");
    words.push(answered(&mut h, still, Duration::from_secs(2)));
    words.push(answered(&mut h, FollowUp { row: None, interrupted: None }, Duration::from_secs(7)));
    assert_eq!(words.last().unwrap(), "Session 4211 has ended");
    for _ in 0..20 {
        h.advance(Duration::from_secs(2));
    }
    assert!(follow_ups(&mut h).is_empty(), "three reads, then none");
    assert_eq!(h.app.monitor_due(), None, "nothing to wake for");
    assert!(words.iter().all(|w| !w.contains("cancelled")), "never cancelled without the server saying so: {words:?}");
    assert!(ticks(&mut h).is_empty(), "hidden and paused: no read of the list");
}

#[test]
fn the_server_saying_the_statement_was_interrupted_is_the_only_cancelled() {
    let mut h = pg_selected(4211);
    h.keys("c");
    h.keys("y");
    let request = actions_sent(&mut h)[0].0;
    h.monitor_db(DbEvent::MonitorActionDone { request, outcome: ActionOutcome::OutcomeUnknown });
    assert_eq!(said(&h), "Sent to session 4211; its answer was lost: the next reads tell");
    h.advance(Duration::from_secs(1));
    let f = follow_ups(&mut h);
    assert_eq!(f.len(), 1);
    h.monitor_db(DbEvent::MonitorFollowUp {
        request: f[0].0,
        result: Ok(FollowUp { row: None, interrupted: Some(true) }),
    });
    assert_eq!(said(&h), "The server says the statement of session 4211 was cancelled");
    h.advance(Duration::from_secs(20));
    assert!(follow_ups(&mut h).is_empty(), "settled: no more reads");
}

#[test]
fn refusals_and_failures_are_said_and_not_followed_up() {
    let cases = [
        (ActionOutcome::NotSent(NotSent::Stale(Stale::SessionGone)), "Nothing sent: session 4211 has ended"),
        (
            ActionOutcome::NotSent(NotSent::Stale(Stale::StatementChanged)),
            "Nothing sent: session 4211 runs another statement now",
        ),
        (
            ActionOutcome::NotSent(NotSent::Refused(Denied::Grant(&["pg_signal_backend"]))),
            "Refused by the server: needs one of pg_signal_backend (or the session's own role)",
        ),
        (ActionOutcome::Failed(DbError::Server("boom".into())), "Not sent: "),
    ];
    for (outcome, text) in cases {
        let mut h = pg_selected(4211);
        h.keys("c");
        h.keys("y");
        let request = actions_sent(&mut h)[0].0;
        h.monitor_db(DbEvent::MonitorActionDone { request, outcome });
        assert!(said(&h).starts_with(text), "{text}: {}", said(&h));
        h.advance(Duration::from_secs(12));
        assert!(follow_ups(&mut h).is_empty(), "{text}");
    }
}

#[test]
fn answers_of_another_request_or_connection_are_dropped_and_a_reconnect_drops_what_was_asked() {
    let mut h = pg_selected(4211);
    h.keys("c");
    h.keys("y");
    let request = actions_sent(&mut h)[0].0;
    // Not the action sent.
    h.monitor_db(DbEvent::MonitorActionDone {
        request: request + 50,
        outcome: ActionOutcome::Acknowledged { via: Via::Standard },
    });
    assert!(monitor(&h).actions[0].answer.is_none());
    assert_ne!(said(&h), "Cancel sent to session 4211");
    // The connection is lost before the answer: an answer of the old connection is dropped.
    let old = monitor(&h).generation;
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    let id = h.app.tab().id;
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target: datarig_tui::app::EventTarget::Monitor(id),
        generation: old,
        ev: DbEvent::MonitorActionDone { request, outcome: ActionOutcome::Acknowledged { via: Via::Standard } },
    });
    assert_ne!(said(&h), "Cancel sent to session 4211");
    let e = &monitor(&h).actions[0];
    assert_eq!(e.answer.as_ref().map(|n| n.msg.clone()), Some(Label::MonitorActLost.into()), "its answer is not known");
    // A dialog open while the monitor reconnects sends nothing on `y`.
    let mut h = pg_selected(4211);
    h.keys("X");
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    h.keys("y");
    assert_eq!(said(&h), "The monitor reconnected: nothing sent");
    assert!(actions_sent(&mut h).is_empty());
    // So does a question of the status line: it is dropped.
    let mut h = pg_selected(4211);
    h.keys("c");
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    assert!(asked(&h).is_none());
    assert_eq!(said(&h), "The monitor reconnected: nothing sent");
    h.keys("y");
    assert!(actions_sent(&mut h).is_empty());
}

/// The index of the first session of `role`.
fn session_of(h: &Harness, role: SessionRole) -> usize {
    h.roles().iter().position(|r| *r == role).expect("a session")
}

/// The server-session asks sent to session `i` since the last look.
fn own_asks(h: &mut Harness, i: usize) -> Vec<u64> {
    h.sent_to(i)
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::ServerSession { request } => Some(request),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tab_that_could_not_tell_its_session_is_asked_again_once_idle() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.tab_db(0, DbEvent::Connected);
    let q = session_of(&h, SessionRole::Query);
    h.sent();
    let (mut h, asks) = sessions_asked(h, pg_format(), rows());
    assert_eq!(asks.len(), 1);
    h.tab_db(0, DbEvent::ServerSession { request: asks[0], own: OwnSession::NotKnown });
    // Not while it runs.
    for _ in 0..3 {
        h.advance(Duration::from_secs(2));
        if let Some(t) = ticks(&mut h).first() {
            h.monitor_db(DbEvent::MonitorSnapshot { request: t.0, result: Ok(snapshot(2.0, 1001, rows(), false)) });
        }
    }
    assert!(own_asks(&mut h, q).is_empty(), "not while it runs");
    // NotKnown marks no row: the cancel is the monitor's.
    select(&mut h, 4211);
    h.keys("c");
    assert!(asked(&h).is_some_and(|a| a.starts_with("Cancel the statement")), "{:?}", asked(&h));
    h.keys("n");
    let id = h.app.tabs.iter().next().unwrap().exec.query_id;
    h.tab_db(
        0,
        DbEvent::Done {
            id,
            outcome: datarig_core::driver::Outcome::Command("SELECT".into()),
            elapsed: Default::default(),
        },
    );
    h.advance(Duration::from_secs(2));
    assert_eq!(own_asks(&mut h, q).len(), 1, "asked again once idle");
}

#[test]
fn the_menu_lists_cancel_and_end_dimmed_with_why() {
    let mut h = pg_selected(3907);
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let dimmed = h.app.menu_dimmed_rows();
    assert!(
        dimmed.iter().any(|(l, why)| l.as_str() == "Monitor: cancel the session's statement" && why == "no statement"),
        "{dimmed:?}"
    );
    let live = h.app.menu_rows();
    assert!(live.iter().any(|(l, _)| l.as_str() == "Monitor: end the session"), "{live:?}");
    assert!(live.iter().any(|(l, _)| l.as_str() == "Monitor: show / hide the action list"), "{live:?}");
}

#[test]
fn the_menu_lists_every_view_and_the_server_pane_too() {
    let mut h = pg_selected(4211);
    h.keys("  ");
    let live = h.app.menu_rows();
    for item in [
        "Monitor: Overview",
        "Monitor: Sessions",
        "Monitor: Locks",
        "Monitor: Replication",
        "Monitor: the server in full",
        "Monitor: cancel the session's statement",
        "Monitor: end the session",
        "Monitor: show / hide the action list",
    ] {
        assert!(live.iter().any(|(l, _)| l.as_str() == item), "{item}: {live:?}");
    }
}

#[test]
fn the_server_pane_over_the_sessions_hides_the_row_from_cancel_and_end() {
    let mut h = pg_selected(4211);
    h.keys("i");
    assert!(monitor(&h).server_info.is_some());
    h.keys("c");
    assert_eq!(said(&h), "Select a session in the Sessions view");
    h.keys("X");
    assert_eq!(said(&h), "Select a session in the Sessions view");
    assert!(asked(&h).is_none());
    assert_eq!(h.overlay_kind(), None);
    assert!(actions_sent(&mut h).is_empty());
    // The menu over the pane offers no action on the hidden row.
    h.keys("  ");
    let live = h.app.menu_rows();
    assert!(!live.iter().any(|(l, _)| l.as_str() == "Monitor: cancel the session's statement"), "{live:?}");
    assert!(!live.iter().any(|(l, _)| l.as_str() == "Monitor: end the session"), "{live:?}");
    h.key(KeyCode::Esc);
    // Closed, the row is there to act on again.
    h.key(KeyCode::Esc);
    assert_eq!(monitor(&h).server_info, None);
    h.keys("c");
    assert!(asked(&h).is_some_and(|a| a.starts_with("Cancel the statement of session 4211")));
}

#[test]
fn screens_of_the_end_dialog_and_the_action_list() {
    let mut h = pg_selected(4211);
    h.keys("X");
    insta::assert_snapshot!("monitor_terminate_modal_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_terminate_modal_80x24", h.screen(80, 24));
    h.keys("y");
    let request = actions_sent(&mut h)[0].0;
    h.monitor_db(DbEvent::MonitorActionDone { request, outcome: ActionOutcome::Acknowledged { via: Via::Standard } });
    h.keys("L");
    let screen = h.screen(120, 40);
    assert!(screen.contains("end sent"), "the row says it: {screen}");
    h.advance(Duration::from_secs(1));
    let f = follow_ups(&mut h);
    h.monitor_db(DbEvent::MonitorFollowUp { request: f[0].0, result: Ok(FollowUp { row: None, interrupted: None }) });
    insta::assert_snapshot!("monitor_actions_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_actions_80x24", h.screen(80, 24));
}

#[test]
fn rows_read_before_a_reconnect_take_no_action_until_read_again() {
    let mut h = pg_selected(4211);
    h.keys("p");
    h.monitor_db(DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    // Connected again and probed while paused: the list shown is the old connection's.
    h.keys("r");
    h.monitor_db(DbEvent::Connected);
    h.monitor_db(DbEvent::Monitor(Ok(pg_format())));
    assert!(matches!(monitor(&h).state, MonState::Ready(_)));
    assert_eq!(monitor(&h).selected_row().map(|r| r.key.id), Some(4211), "the old list is still shown");
    let stale = "The list is from before the monitor reconnected: read it again first (r)";
    h.keys("c");
    assert!(asked(&h).is_none(), "{:?}", asked(&h));
    assert_eq!(said(&h), stale);
    h.keys("X");
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(said(&h), stale);
    assert!(actions_sent(&mut h).is_empty());
    // The menu says so too.
    h.keys("  ");
    let dimmed = h.app.menu_dimmed_rows();
    assert!(dimmed.iter().any(|(l, _)| l.as_str() == "Monitor: end the session"), "{dimmed:?}");
    h.key(KeyCode::Esc);
    // Read again on the new connection: offered again.
    answer_read(&mut h, snapshot(10.0, 1100, rows(), true));
    select(&mut h, 4211);
    h.keys("c");
    assert!(asked(&h).is_some_and(|a| a.starts_with("Cancel the statement of session 4211")), "{:?}", asked(&h));
}

/// The active Monitor tab's read in flight (the next one, when none is) answered with `s`.
fn answer_read(h: &mut Harness, s: Snapshot) {
    if monitor(h).request.is_none() {
        h.advance(Duration::from_secs(2));
    }
    let (request, _) = monitor(h).request.expect("a read in flight");
    h.monitor_db(DbEvent::MonitorSnapshot { request, result: Ok(s) });
}

/// The commands sent to session `i` since the last look, as ServerSession / Execute / other.
fn kinds_sent_to(h: &mut Harness, i: usize) -> Vec<&'static str> {
    h.sent_to(i)
        .into_iter()
        .map(|c| match c {
            DbCommand::ServerSession { .. } => "ServerSession",
            DbCommand::Execute { .. } => "Execute",
            _ => "other",
        })
        .collect()
}

/// The Monitor tab open on the usual sessions, then the first tab runs `text` (its statements
/// selected) on a session it opens for it: asked which server session it is right before the
/// run, the answer (server session 4211) comes before the run starts, as the driver answers in
/// order. Back in the Monitor tab with 4211 selected; the query session's index and the run id.
fn own_tab_running(text: &str) -> (Harness, usize, u64) {
    own_tab_running_on(text, pg_format(), rows)
}

/// [`own_tab_running`] with a monitor of `format` listing `rows()` (session 4211 listing `text`).
fn own_tab_running_on(text: &str, format: MonitorFormat, rows: fn() -> Vec<SessionRow>) -> (Harness, usize, u64) {
    let mut h = Harness::connected(Lang::En);
    h.app.set_instance_tag("t");
    let mut h = sessions_on(h, format, rows());
    h.key_mod(KeyCode::PageUp, ratatui::crossterm::event::KeyModifiers::CONTROL);
    assert_eq!(h.app.tab().kind, TabKind::Console);
    h.app.focus = Focus::Editor;
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(text);
    h.keys("ggvG$");
    h.ctrl('e');
    let q = session_of(&h, SessionRole::Query);
    let mut asks = Vec::new();
    let mut kinds = Vec::new();
    for c in h.sent_to(q) {
        match c {
            DbCommand::ServerSession { request } => {
                asks.push(request);
                kinds.push("ServerSession");
            }
            DbCommand::Execute { .. } => kinds.push("Execute"),
            _ => {}
        }
    }
    assert_eq!(kinds, ["ServerSession", "Execute"], "asked right before the run");
    h.tab_db(0, DbEvent::Connected);
    h.tab_db(0, DbEvent::ServerSession { request: asks[0], own: OwnSession::Direct { id: 4211 } });
    let id = h.app.tabs.iter().next().unwrap().exec.query_id;
    h.tab_db(0, DbEvent::Started { id, index: 0 });
    h.advance(Duration::from_millis(10));
    // Shown again: read at once, after the statement started.
    h.key_mod(KeyCode::PageDown, ratatui::crossterm::event::KeyModifiers::CONTROL);
    assert_eq!(h.app.tab().kind, TabKind::Monitor);
    answer_read(&mut h, snapshot(2.0, 1001, running(rows(), text), false));
    select(&mut h, 4211);
    h.sent();
    (h, q, id)
}

/// The session details on `screen` have the field naming the tab ("tab  console 1").
fn tab_field(screen: &str) -> bool {
    screen.lines().any(|l| {
        l.split('│').any(|cell| {
            let mut words = cell.split_whitespace();
            words.next() == Some("tab") && words.collect::<Vec<_>>() == ["console", "1"]
        })
    })
}

/// The application name the app's query sessions connect with (instance tag `t`).
const QUERY_APP: &str = "datarig-q-t";

/// `rows` with session 4211 as the tab's own session shows: listing `text` as its statement,
/// as the profile's user in its database, with the query sessions' application name.
fn running(mut rows: Vec<SessionRow>, text: &str) -> Vec<SessionRow> {
    for r in rows.iter_mut().filter(|r| r.key.id == 4211) {
        r.query = Some(QueryText { text: text.into(), shortened: false, maybe_cut_by_server: false });
        r.user = Some("datarig".into());
        r.database = Some("datarig".into());
        r.application = Some(QUERY_APP.into());
    }
    rows
}

#[test]
fn a_run_asks_its_server_session_first_only_while_a_monitor_tab_of_the_profile_is_open() {
    // A Monitor tab open (hidden): the run's session is asked first, then runs.
    let (mut h, q, id) = own_tab_running("SELECT 1");
    h.tab_db(
        0,
        DbEvent::Done {
            id,
            outcome: datarig_core::driver::Outcome::Command("SELECT".into()),
            elapsed: Default::default(),
        },
    );
    h.sent();
    // Known: the next run asks nothing.
    h.key_mod(KeyCode::PageUp, ratatui::crossterm::event::KeyModifiers::CONTROL);
    h.app.focus = Focus::Editor;
    h.ctrl('e');
    assert_eq!(kinds_sent_to(&mut h, q), ["Execute"]);
    // No Monitor tab: nothing extra at a run.
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let q = session_of(&h, SessionRole::Query);
    assert_eq!(kinds_sent_to(&mut h, q), ["Execute"], "no ServerSession without a Monitor tab");
}

#[test]
fn a_right_click_on_a_session_acts_on_that_session() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = pg_selected(3907);
    h.screen(120, 40);
    let line = |h: &Harness, id: i64| {
        let m = monitor(h);
        let list = &m.snapshot.as_ref().unwrap().sessions.as_ref().unwrap().rows;
        let at = m.list.order.iter().position(|&i| list[i].key.id == id).unwrap();
        m.hits.rows.y + (at - m.list.top) as u16
    };
    let (x, y) = (monitor(&h).hits.rows.x + 3, line(&h, 4302));
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, y);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    assert_eq!(monitor(&h).selected_row().map(|r| r.key.id), Some(4302), "the row under the pointer");
    h.type_text("end the");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(120, 40).contains("Session 4302: web on shop"));
    h.keys("y");
    let sent = actions_sent(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].2.key.id, 4302);
    // Elsewhere: the selection stays.
    h.screen(120, 40);
    let below = monitor(&h).hits.rows.y + monitor(&h).hits.rows.height - 1;
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, below);
    assert_eq!(monitor(&h).selected_row().map(|r| r.key.id), Some(4302));
}

#[test]
fn a_menu_item_picked_after_the_selection_moved_runs_nothing() {
    let mut h = pg_selected(4211);
    // A read goes out, then the menu opens on 4211.
    h.advance(Duration::from_secs(2));
    let (request, _) = monitor(&h).request.expect("a read in flight");
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    // Its answer: 4211 has gone, the selection moves.
    let without: Vec<SessionRow> = rows().into_iter().filter(|r| r.key.id != 4211).collect();
    h.monitor_db(DbEvent::MonitorSnapshot { request, result: Ok(snapshot(2.0, 1001, without, false)) });
    assert_ne!(monitor(&h).selected_row().map(|r| r.key.id), Some(4211));
    h.type_text("end the");
    h.key(KeyCode::Enter);
    assert_ne!(h.overlay_kind(), Some(OverlayKind::Confirm), "no dialog for another session");
    assert_eq!(said(&h), "Not run: what the menu was opened on has changed. Open the menu again");
    assert!(actions_sent(&mut h).is_empty());
}

#[test]
fn your_own_tabs_session_is_named_and_cancelled_by_the_guarded_cancel() {
    let (mut h, q, _) = own_tab_running("SELECT pg_sleep(60)");
    h.key(KeyCode::Enter);
    assert!(tab_field(&h.screen(120, 40)), "the details name the tab");
    h.key(KeyCode::Esc);
    h.keys("c");
    let a = asked(&h).expect("asked");
    assert_eq!(a, "Session 4211 is your tab 'console 1'. Cancel its statement? y/n");
    h.keys("y");
    let row = monitor(&h).selected_row().unwrap().clone();
    let sent = actions_sent(&mut h);
    assert_eq!(sent.len(), 1, "the monitor's guarded cancel: {sent:?}");
    assert_eq!(sent[0].1, ActionKind::Cancel);
    assert_eq!(sent[0].2.key, row.key);
    assert_eq!(sent[0].2.stamp, row.statement, "it names the statement");
    let t = h.app.tabs.iter().next().unwrap();
    assert!(t.exec.running.unwrap().cancelling.is_none(), "the tab's own cancel is never used");
    assert!(!h.session_cancelled(q));
    // Lost: no row is the tab's any more; the question is the usual one.
    h.tab_db(0, DbEvent::Lost { error: DbError::Connection(datarig_core::fault::Fault::other("reset")) });
    h.keys("c");
    assert!(asked(&h).is_some_and(|a| a.starts_with("Cancel the statement of session 4211")), "{:?}", asked(&h));
    h.keys("n");
}

#[test]
fn your_own_tabs_session_is_refused_as_any_other_when_it_runs_nothing() {
    let (mut h, q, _) = own_tab_running("SELECT pg_sleep(60)");
    let mut idle = running(rows(), "SELECT pg_sleep(60)");
    idle[0].state = SessionState::Idle;
    answer_read(&mut h, snapshot(4.0, 1002, idle, false));
    select(&mut h, 4211);
    h.keys("c");
    assert!(asked(&h).is_none(), "{:?}", asked(&h));
    assert_eq!(said(&h), "Session 4211 runs no statement");
    assert!(actions_sent(&mut h).is_empty());
    assert!(!h.session_cancelled(q));
}

#[test]
fn a_mysql_dialog_never_names_your_own_tab_as_the_list_shows_no_application() {
    let (mut h, q, _) =
        own_tab_running_on("UPDATE orders SET status = 'paid' WHERE id = 7", mysql_format(), || mysql_rows("SELECT 1"));
    // As MySQL lists it: no application, so nothing tells this session is the tab's.
    let mut listed = running(mysql_rows("SELECT 1"), "UPDATE orders SET status = 'paid' WHERE id = 7");
    listed[0].application = None;
    answer_read(&mut h, snapshot(4.0, 1002, listed, false));
    select(&mut h, 4211);
    h.keys("c");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let details = &h.app.overlays.confirm().unwrap().details;
    assert!(details.contains(&Label::MonitorActWarnHalfDone.into()), "{details:?}");
    assert!(
        !details.contains(&datarig_core::i18n::Msg::MonitorActOwnTabCancel { tab: "console 1".into() }),
        "no own-tab line: {details:?}"
    );
    h.keys("y");
    let sent = actions_sent(&mut h);
    assert_eq!(sent.len(), 1, "the guarded cancel: {sent:?}");
    assert_eq!(sent[0].1, ActionKind::Cancel);
    assert_eq!(sent[0].2.key.id, 4211);
    assert!(!h.session_cancelled(q));
}

#[test]
fn a_session_with_the_tabs_id_but_not_its_user_database_or_application_is_not_named_the_tab() {
    // The id the tab's connection reported, but the row is not consistent with that session
    // (another server behind the same endpoint, say): the plain question, no tab named.
    /// What a case changes in the row.
    type Change = fn(&mut SessionRow);
    let cases: [(&str, Change); 4] = [
        ("user", |r| r.user = Some("alice".into())),
        ("database", |r| r.database = Some("shop".into())),
        ("application", |r| r.application = Some("psql".into())),
        ("no application", |r| r.application = None),
    ];
    for (what, change) in cases {
        let (mut h, q, _) = own_tab_running("SELECT pg_sleep(60)");
        let mut other = running(rows(), "SELECT pg_sleep(60)");
        change(&mut other[0]);
        answer_read(&mut h, snapshot(4.0, 1002, other, false));
        select(&mut h, 4211);
        h.key(KeyCode::Enter);
        let screen = h.screen(120, 40);
        assert!(!tab_field(&screen), "{what}: the details name no tab: {screen}");
        h.key(KeyCode::Esc);
        h.keys("c");
        let a = asked(&h).expect("asked");
        assert!(a.starts_with("Cancel the statement of session 4211"), "{what}: {a}");
        h.keys("n");
        assert!(!h.session_cancelled(q));
    }
}

/// A statement whole in the list and long enough to wrap: a report over the orders.
const REPORT: &str = "SELECT o.id, o.status, sum(i.qty * i.price) AS total FROM orders o JOIN order_items i \
                      ON i.order_id = o.id WHERE o.status = 'paid' GROUP BY o.id, o.status ORDER BY total DESC \
                      LIMIT 20";

/// The usual PostgreSQL sessions, session 4302 running `text` (as `shortened` and
/// `maybe_cut_by_server` say), its details open.
fn statement_details(text: &str, shortened: bool, maybe_cut_by_server: bool) -> Harness {
    let mut rows = rows();
    let r = rows.iter_mut().find(|r| r.key.id == 4302).unwrap();
    r.query = Some(QueryText { text: text.into(), shortened, maybe_cut_by_server });
    let mut h = sessions_on(Harness::connected(Lang::En), pg_format(), rows);
    select(&mut h, 4302);
    h.key(KeyCode::Enter);
    assert!(monitor(&h).list.details);
    h
}

/// The lines of the details' statement on screen: from the line after the layout's note to the
/// bottom border, without the frame and blanks around them.
fn statement_on(screen: &str, note: &str) -> Vec<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let at = lines.iter().position(|l| l.contains(note)).unwrap_or_else(|| panic!("{note:?} in\n{screen}"));
    lines[at + 1..]
        .iter()
        .take_while(|l| !l.contains('╰'))
        .map(|l| l.rsplit("││").next().unwrap_or(l).trim_end_matches('│').trim_end().to_string())
        .filter(|l| !l.trim().is_empty())
        .collect()
}

#[test]
fn the_details_lay_a_statement_out_for_reading_and_f_shows_it_as_sent_80x24_and_120x40() {
    let mut h = statement_details(REPORT, false, false);
    // All of it on a taller screen.
    let screen = h.screen(120, 60);
    let shown = statement_on(&screen, "Laid out for reading (f: as sent)");
    assert_eq!(shown.first().map(|l| l.trim()), Some("SELECT"), "{screen}");
    for clause in ["FROM", "JOIN", "WHERE", "GROUP BY", "ORDER BY", "LIMIT"] {
        assert!(shown.iter().any(|l| l.trim().starts_with(clause)), "{clause} on a line of its own:\n{screen}");
    }
    // Only the blanks changed.
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(words(&shown.join(" ")), words(REPORT));
    insta::assert_snapshot!("monitor_statement_laid_out_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_statement_laid_out_80x24", h.screen(80, 24));
    h.keys("f");
    assert_eq!(said(&h), "Statements as the server sent them");
    let screen = h.screen(120, 40);
    let shown = statement_on(&screen, "As the server sent it (f: laid out)");
    assert!(
        shown[0].trim().starts_with("SELECT o.id, o.status, sum(i.qty * i.price) AS total FROM orders o"),
        "{screen}"
    );
    assert_eq!(shown.join("").replace(' ', ""), REPORT.replace(' ', ""), "the server's text, wrapped:\n{screen}");
    insta::assert_snapshot!("monitor_statement_as_sent_120x40", screen);
    insta::assert_snapshot!("monitor_statement_as_sent_80x24", h.screen(80, 24));
    // The choice stays for another session's details, and `f` again lays it out.
    h.key(KeyCode::Esc);
    select(&mut h, 4211);
    h.key(KeyCode::Enter);
    assert!(h.screen(120, 40).contains("As the server sent it (f: laid out)"));
    h.keys("f");
    assert_eq!(said(&h), "Statements laid out for reading (copying takes the server's text)");
    assert!(h.screen(120, 40).contains("Laid out for reading (f: as sent)"));
}

#[test]
fn the_statement_is_drawn_in_the_editors_syntax_colours() {
    let mut h = statement_details(REPORT, false, false);
    for f in ["", "f"] {
        h.keys(f);
        let t = h.draw(120, 60);
        let buf = t.backend().buffer();
        let th = datarig_tui::theme::cur();
        let area = *buf.area();
        let rows: Vec<String> =
            (0..area.height).map(|y| (0..area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
        // A word's first cell in the details (the lower half: the list is above).
        let cell = |word: &str| {
            let (y, x) = rows
                .iter()
                .enumerate()
                .skip(20)
                .find_map(|(y, r)| r.find(word).map(|b| (y, r[..b].chars().count())))
                .unwrap_or_else(|| panic!("{word:?} in the details\n{}", rows.join("\n")));
            buf[(x as u16, y as u16)].clone()
        };
        let kw = cell("JOIN");
        assert_eq!(kw.fg, th.syn_keyword.fg.unwrap(), "{f:?}: a keyword");
        assert!(kw.modifier.contains(ratatui::style::Modifier::BOLD), "{f:?}");
        assert_eq!(cell("'paid'").fg, th.syn_string.fg.unwrap(), "{f:?}: a string");
        assert_eq!(cell("sum(").fg, th.syn_function.fg.unwrap(), "{f:?}: a call");
        assert_eq!(cell("20").fg, th.syn_number.fg.unwrap(), "{f:?}: a number");
    }
}

#[test]
fn a_statement_known_only_in_part_is_shown_as_sent_with_its_cut_note() {
    // The list's start, while the whole text is read.
    let (mut h, request) = long_details();
    let screen = h.screen(120, 40);
    assert!(screen.contains("The list holds the first 256 characters of the statement"), "{screen}");
    let shown = statement_on(&screen, "Not laid out: only the start of the statement is known");
    assert_eq!(shown.len(), 1, "{screen}");
    assert!(shown[0].trim().ends_with(START), "{screen}");
    // The whole text read: laid out.
    text_answer(&mut h, request, Ok(whole(Proof::Same)));
    let screen = h.screen(120, 40);
    let shown = statement_on(&screen, "Laid out for reading");
    assert!(shown.len() > 1, "{screen}");
    // Cut by the server: as sent, under its note.
    let mut h = statement_details(REPORT, false, true);
    let screen = h.screen(120, 40);
    assert!(screen.contains("The server may have cut this statement where it keeps it"), "{screen}");
    let shown = statement_on(&screen, "Not laid out: only the start of the statement is known");
    assert!(shown[0].trim().starts_with("SELECT o.id, o.status, sum("), "{screen}");
    insta::assert_snapshot!("monitor_statement_cut_120x40", screen);
    insta::assert_snapshot!("monitor_statement_cut_80x24", h.screen(80, 24));
}

#[test]
fn a_statement_the_formatter_would_change_is_shown_as_sent() {
    // A Unicode escape string: a layout would change more than its blanks.
    let text = "SELECT U&'d\\0061t' FROM t WHERE a = 1";
    let mut h = statement_details(text, false, false);
    let screen = h.screen(120, 40);
    let shown = statement_on(&screen, "Not laid out: a layout would change more than its blanks");
    assert_eq!(shown.iter().map(|l| l.trim()).collect::<Vec<_>>(), [text], "{screen}");
    insta::assert_snapshot!("monitor_statement_refused_80x24", h.screen(80, 24));
    // Over 64 KiB: as sent, not laid out at all.
    let long = format!("SELECT {} FROM t", vec!["a"; 40_000].join(", "));
    let mut h = statement_details(&long, false, false);
    assert!(h.screen(120, 40).contains("Not laid out: longer than 64 KiB"));
    assert_eq!(monitor(&h).statement.as_ref().map(|s| s.text.as_str()), Some(long.as_str()));
}

#[test]
fn copying_and_opening_take_the_servers_text_not_the_layout() {
    let mut h = statement_details(REPORT, false, false);
    assert!(h.screen(120, 40).contains("Laid out for reading"));
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some(REPORT));
    h.keys("o");
    assert_eq!(h.app.tab().kind, TabKind::Console);
    assert_eq!(h.app.tab().editor.text(), REPORT);
}

#[test]
fn a_statement_is_laid_out_once_not_on_every_frame() {
    let mut h = statement_details(REPORT, false, false);
    h.screen(120, 40);
    let first = monitor(&h).statement.as_ref().map(|s| s.text.as_ptr());
    for _ in 0..3 {
        h.screen(120, 40);
        h.keys("j");
    }
    assert_eq!(monitor(&h).statement.as_ref().map(|s| s.text.as_ptr()), first, "worked out once");
    // A new read with the same statement keeps it too.
    h.advance(Duration::from_secs(2));
    let t = ticks(&mut h);
    let mut rows = rows();
    rows.iter_mut().find(|r| r.key.id == 4302).unwrap().query =
        Some(QueryText { text: REPORT.into(), shortened: false, maybe_cut_by_server: false });
    h.monitor_db(DbEvent::MonitorSnapshot { request: t[0].0, result: Ok(snapshot(2.0, 1100, rows, false)) });
    h.screen(120, 40);
    assert_eq!(monitor(&h).statement.as_ref().map(|s| s.text.as_ptr()), first, "the same text");
}

#[test]
fn the_monitors_own_session_is_named_in_its_details_on_postgresql() {
    let mut h = pg_selected(4400);
    let list = h.screen(120, 40);
    assert!(list.lines().any(|l| l.contains("4400") && l.contains("this monitor")), "{list}");
    h.key(KeyCode::Enter);
    let screen = h.screen(120, 40);
    assert!(screen.contains("session            4400 (this monitor)"), "{screen}");
    assert!(
        screen.contains("The monitor's own connection: its statement is the monitor reading the server"),
        "{screen}"
    );
    insta::assert_snapshot!("monitor_own_session_pg_120x40", screen);
    insta::assert_snapshot!("monitor_own_session_pg_80x24", h.screen(80, 24));
    h.keys("c");
    assert_eq!(said(&h), "This is the monitor's own connection");
    assert!(actions_sent(&mut h).is_empty());
}

/// The monitor's own read on MySQL, as the process list shows it to an account without PROCESS.
const MYSQL_OWN: &str = "SELECT CAST(UNIX_TIMESTAMP(NOW(6)) * 1000000 AS SIGNED) AS now, p.ID, p.USER, p.HOST, \
                         p.DB, p.COMMAND, p.TIME, p.STATE, LEFT(p.INFO, 256) AS INFO, p.ID = CONNECTION_ID() AS \
                         monitor FROM information_schema.PROCESSLIST p ORDER BY p.ID";

#[test]
fn the_monitors_own_session_is_named_in_its_details_on_mysql() {
    use datarig_core::config::Config;
    use datarig_core::monitor::format::{MySqlMonitor, Readable};
    use datarig_core::profile::ConnectionConfig;
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let cfg = Config {
        connections: vec![ConnectionConfig { driver: "mysql".into(), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    // Without PROCESS: only the account's own sessions, the monitor's among them.
    let format = MonitorFormat::MySql(MySqlMonitor {
        version: 80411,
        performance_schema: true,
        user: "datarig".into(),
        process: Readable::Refused,
        innodb_trx: Readable::Refused,
        metadata_locks: Readable::Yes,
        statements: Readable::Yes,
        flavour: Flavour::Standard,
    });
    let mut rows = mysql_rows("SELECT 1");
    rows.iter_mut().find(|r| r.monitor).unwrap().query =
        Some(QueryText { text: MYSQL_OWN.into(), shortened: false, maybe_cut_by_server: false });
    let mut h = sessions_on(h, format, rows);
    select(&mut h, 4400);
    let list = h.screen(120, 40);
    assert!(list.lines().any(|l| l.contains("4400") && l.contains("this monitor")), "{list}");
    h.key(KeyCode::Enter);
    let screen = h.screen(120, 40);
    assert!(screen.contains("4400 (this monitor)"), "{screen}");
    assert!(screen.contains("The monitor's own connection"), "{screen}");
    // Laid out in MySQL's dialect (all of it on a taller screen).
    let shown = statement_on(&h.screen(120, 60), "Laid out for reading");
    assert!(shown.iter().any(|l| l.trim().starts_with("FROM")), "{screen}");
    insta::assert_snapshot!("monitor_own_session_mysql_120x40", screen);
    insta::assert_snapshot!("monitor_own_session_mysql_80x24", h.screen(80, 24));
    h.keys("X");
    assert_eq!(said(&h), "This is the monitor's own connection");
    assert!(actions_sent(&mut h).is_empty());
}

/// MySQL with a session that said it reads `ANSI_QUOTES`: session 4302's statement `text`, its
/// details open.
fn mysql_ansi_details(text: &str) -> Harness {
    use datarig_core::config::Config;
    use datarig_core::profile::ConnectionConfig;
    use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let cfg = Config {
        connections: vec![ConnectionConfig { driver: "mysql".into(), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    let ansi = MySqlMode { ansi_quotes: true, ..MySqlMode::default() };
    h.db(DbEvent::Language(Language::Sql(Dialect::MySql(ansi))));
    let mut h = sessions_on(h, mysql_format(), mysql_rows(text));
    select(&mut h, 4302);
    h.key(KeyCode::Enter);
    h
}

#[test]
fn the_statement_is_read_in_the_sql_mode_the_sessions_said() {
    // Under ANSI_QUOTES "total" is a name, not a string.
    let mut h = mysql_ansi_details("SELECT \"total\" FROM t WHERE a = 'x'");
    let t = h.draw(120, 60);
    let buf = t.backend().buffer();
    let th = datarig_tui::theme::cur();
    let area = *buf.area();
    let rows: Vec<String> =
        (0..area.height).map(|y| (0..area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
    let cell = |word: &str| {
        let (y, x) = rows
            .iter()
            .enumerate()
            .skip(14)
            .find_map(|(y, r)| r.find(word).map(|b| (y, r[..b].chars().count())))
            .unwrap_or_else(|| panic!("{word:?} in the details\n{}", rows.join("\n")));
        buf[(x as u16, y as u16)].clone()
    };
    assert_eq!(cell("\"total\"").fg, th.syn_quoted_ident.fg.unwrap(), "a quoted name");
    assert_eq!(cell("'x'").fg, th.syn_string.fg.unwrap(), "a string");
}
