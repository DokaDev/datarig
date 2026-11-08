//! The Monitor tab of a busy server: 5,000 sessions (none idle: every one is listed), a new read
//! every tenth frame, at 160x45 while the selection moves, in the Sessions view and the
//! Overview. Measured: the key (or the read's arrival) with its frame, the work of one frame
//! (`widgets::monitor`'s count: session rows and chart tiles drawn), and taking a read (the
//! rates, the sort and the filter of its sessions). No server: the reads are made here and fed
//! to the tab as its connection would.

use crate::apps;
use crate::stats::{Summary, ms};
use datarig_core::driver::DbEvent;
use datarig_core::monitor::{
    Counter, CounterKind, Epoch, Gauge, GaugeKind, Own, QueryText, Sample, ServerInfo, SessionKey, SessionKind,
    SessionList, SessionRow, SessionState, Snapshot, StatementStamp, Timestamp, Tx, Visibility, Wait, WaitClass,
};
use datarig_tui::app::monitor::{MonState, MonitorTab};
use datarig_tui::app::{AppEvent, EventTarget, Focus, TabKind};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use std::time::Instant;

/// The server's clock at read `n` (two seconds apart).
fn time(n: u64) -> Timestamp {
    Timestamp(1_794_060_000_000_000 + n as i64 * 2_000_000)
}

/// Read `n` of a server with `sessions` sessions, all busy (running, waiting, or idle in a
/// transaction), each with a statement of 200 characters.
fn read(n: u64, sessions: usize) -> Snapshot {
    let now = time(n);
    let rows = (0..sessions)
        .map(|i| {
            let (state, wait) = match i % 10 {
                0 => (SessionState::IdleInTx, None),
                1 => (SessionState::Active, Some(Wait { class: WaitClass::Lock, name: "transactionid".into() })),
                2 => (SessionState::Active, Some(Wait { class: WaitClass::Io, name: "DataFileRead".into() })),
                _ => (SessionState::Active, None),
            };
            let age = ((i * 7919) % 600_000) as i64 * 1000;
            SessionRow {
                key: SessionKey { id: 10_000 + i as i64, started: Some(Timestamp(now.0 - 86_400_000_000)) },
                kind: SessionKind::Client,
                user: Some(format!("user{}", i % 50)),
                database: Some(format!("db{}", i % 5)),
                application: Some(format!("app-{}", i % 20)),
                client: Some(format!("10.0.{}.{}:5{:04}", i % 250, i % 200, i % 10_000)),
                state,
                wait,
                tx: Tx::Since(Timestamp(now.0 - age - 1000)),
                query_started: Some(Timestamp(now.0 - age)),
                state_since: Some(Timestamp(now.0 - age)),
                query: Some(QueryText {
                    text: format!("SELECT o.id, o.status, sum(i.amount) FROM orders o JOIN items i ON i.order_id = o.id WHERE o.customer_id = {i} AND o.created_at > now() - interval '30 days' GROUP BY o.id, o.status ORDER BY 3 DESC LIMIT 50"),
                    shortened: false,
                    maybe_cut_by_server: false,
                }),
                blocked_by: None,
                monitor: false,
                visible: Visibility::Full,
                extra: Vec::new(),
                thread: None,
                statement: Some(StatementStamp::Pg { query_start: Some(Timestamp(now.0 - age)) }),
            }
        })
        .collect();
    let c = |kind, per_read: u64| Counter { kind, value: 1_000_000 + n * per_read };
    Snapshot {
        sample: Sample {
            time: now,
            epoch: Epoch::Mark(1, 5),
            counters: vec![
                c(CounterKind::Commits, 8_000 + (n % 7) * 300),
                c(CounterKind::Rollbacks, 20),
                c(CounterKind::RowsRead, 900_000),
                c(CounterKind::RowsWritten, 30_000),
                c(CounterKind::PageRequests, 2_000_000),
                c(CounterKind::PageReads, 9_000 + (n % 5) * 1000),
                c(CounterKind::Deadlocks, 0),
                c(CounterKind::TempBytes, 1_000_000),
            ],
            own: Own::default(),
        },
        gauges: vec![
            Gauge { kind: GaugeKind::Sessions, value: sessions as u64 },
            Gauge { kind: GaugeKind::Active, value: sessions as u64 * 9 / 10 },
            Gauge { kind: GaugeKind::IdleInTx, value: sessions as u64 / 10 },
            Gauge { kind: GaugeKind::Idle, value: 0 },
            Gauge { kind: GaugeKind::WaitingForLock, value: sessions as u64 / 10 },
            Gauge { kind: GaugeKind::Hidden, value: 0 },
        ],
        sessions: Some(SessionList { time: now, rows, idle_left_out: true }),
        server: (n == 0).then(|| ServerInfo {
            version: "PostgreSQL 17.11".into(),
            started: Some(Timestamp(now.0 - 3_600_000_000)),
            read_at: now,
            read_only: false,
            max_connections: Some(10_000),
            cache_bytes: None,
            extra: Vec::new(),
        }),
        hidden: Vec::new(),
    }
}

pub fn run(sessions: usize, n: usize) -> Result<Value, String> {
    let mut app = apps::offline("", None);
    let pid = app.profiles[0].id;
    let tab = app.tabs.open(TabKind::Monitor, Some(pid), Editor::read_only(""));
    let mut m = MonitorTab::new(Default::default());
    // As a probed connection would be, without a server: the reads below are fed to it.
    m.state = MonState::Opening;
    if let Some(t) = app.tabs.get_mut(tab) {
        t.doc.monitor = Some(m);
    }
    app.focus = Focus::Results;
    let mut reads = 0u64;
    let mut take = Vec::new();
    // Feed read `reads` as the answer to the read in flight; the time it took.
    let feed = |app: &mut datarig_tui::app::App, reads: &mut u64| -> f64 {
        let snapshot = read(*reads, sessions);
        let generation = {
            let m = app.tabs.get_mut(tab).and_then(|t| t.doc.monitor.as_mut()).expect("the Monitor tab");
            m.state = MonState::Opening;
            m.request = Some((*reads + 1, Instant::now()));
            m.generation
        };
        let ev = DbEvent::MonitorSnapshot { request: *reads + 1, result: Ok(snapshot) };
        *reads += 1;
        let t = Instant::now();
        app.on_app_event(AppEvent::Db { target: EventTarget::Monitor(tab), generation, ev });
        ms(t.elapsed())
    };
    let mut term = apps::terminal();
    take.push(feed(&mut app, &mut reads));
    apps::draw(&mut term, &mut app);
    let mut views = serde_json::Map::new();
    let (mut all, mut worst) = (Vec::new(), 0u64);
    for (key, view) in [('2', "sessions"), ('1', "overview")] {
        apps::char(&mut app, key);
        apps::draw(&mut term, &mut app);
        datarig_tui::widgets::monitor::take_work();
        let mut frame = Vec::new();
        let mut work = Vec::new();
        for i in 0..n {
            let t = Instant::now();
            if i % 10 == 9 {
                take.push(feed(&mut app, &mut reads));
            } else {
                let code = match i % 10 {
                    0..=4 => KeyCode::Char('j'),
                    5 => KeyCode::Char('G'),
                    6 | 7 => KeyCode::Char('k'),
                    _ => KeyCode::Home,
                };
                apps::key(&mut app, code, KeyModifiers::NONE);
            }
            apps::draw(&mut term, &mut app);
            frame.push(ms(t.elapsed()));
            let w = datarig_tui::widgets::monitor::take_work();
            worst = worst.max(w);
            work.push(w as f64);
        }
        let f = Summary::of(&frame);
        let w = Summary::of(&work);
        println!("monitor {view}: {sessions} sessions at {}x{}", apps::WIDTH, apps::HEIGHT);
        println!("  key or read + frame  {}", f.line(" ms"));
        println!("  work per frame  {}", w.line(""));
        views.insert(view.into(), json!({ "frame_ms": f.json(), "work": w.json() }));
        all.extend(frame);
    }
    if app.monitor_of(tab).and_then(|m| m.snapshot.as_ref()).is_none() {
        return Err("no read was taken".into());
    }
    let f = Summary::of(&all);
    let t = Summary::of(&take);
    println!("  take a read  {}", t.line(" ms"));
    Ok(json!({
        "sessions": sessions,
        "frame_ms": f.json(),
        "work_max": worst,
        "take_ms": t.json(),
        "views": views,
    }))
}
