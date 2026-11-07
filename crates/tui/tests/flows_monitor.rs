//! The Monitor tab: `:monitor` (`M` in the explorer, the connection's menu) opens one per
//! profile, which opens a monitor connection of its own once the profile is connected and reads
//! only while it is the active tab: one read at a time, the next one an interval after the
//! answer; a hidden tab sends nothing. Answers of an older read or connection are dropped. A
//! fake driver records what is sent and the test feeds the answers.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbError, DbEvent, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::monitor::format::{Flavour, PgMonitor};
use datarig_core::monitor::{
    Counter, CounterKind, Epoch, Gauge, GaugeKind, Hidden, MonitorFormat, Need, Own, Part, QueryText, Sample,
    ServerInfo, SessionKey, SessionKind, SessionList, SessionRow, SessionState, Snapshot, Timestamp, Tx, Visibility,
    Wait, WaitClass,
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
        ],
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
    insta::assert_snapshot!("monitor_overview_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_overview_80x24", h.screen(80, 24));
    h.keys("2");
    insta::assert_snapshot!("monitor_sessions_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_sessions_80x24", h.screen(80, 24));
    h.keys("j");
    h.key(KeyCode::Enter);
    insta::assert_snapshot!("monitor_details_120x40", h.screen(120, 40));
    insta::assert_snapshot!("monitor_details_80x24", h.screen(80, 24));
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
        sample: Sample { own: Own { statements: 0, transactions: own }, ..s.sample },
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
