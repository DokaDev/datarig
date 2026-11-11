use super::*;
use std::path::Path;

/// A writer whose writes wait for `gate` and are recorded in `written` (their texts).
fn gated(gate: &Arc<Mutex<()>>, written: &Arc<Mutex<Vec<String>>>) -> Writer {
    let (gate, written) = (gate.clone(), written.clone());
    Writer::spawn(
        "datarig-filter-history",
        Box::new(move |_: &Path, text: &str| {
            let _open = gate.lock().unwrap_or_else(PoisonError::into_inner);
            written.lock().unwrap().push(text.to_string());
            Ok(())
        }),
    )
    .unwrap()
}

/// While a write is stuck, the texts sent behind it never pile up: one waits, the latest, and
/// it is written after the stuck one (never an older text after a newer one).
#[test]
fn a_stuck_write_keeps_one_text_waiting_the_latest() {
    let (gate, written) = (Arc::new(Mutex::new(())), Arc::new(Mutex::new(Vec::new())));
    let held = gate.lock().unwrap();
    let writer = gated(&gate, &written);
    let (tx, rx) = mpsc::channel();
    let write = |seq: u64| Write {
        path: PathBuf::from("history"),
        text: seq.to_string(),
        seq,
        out: Out::Local(tx.clone()),
        log: None,
        event: AppEvent::FilterHistory,
        context: "filter_history.save_failed",
    };
    assert!(writer.write(write(1)).is_none());
    // The writer took it and waits at the gate.
    let start = Instant::now();
    while writer.waiting() > 0 {
        assert!(start.elapsed() < Duration::from_secs(5), "the writer never took the first write");
        std::thread::yield_now();
    }
    let mut replaced = Vec::new();
    for seq in 2..=1000 {
        replaced.extend(writer.write(write(seq)).map(|w| w.seq));
        assert!(writer.waiting() <= 1, "{} writes waiting", writer.waiting());
    }
    assert_eq!(replaced, (2..1000).collect::<Vec<_>>(), "each replaced by the next");
    drop(held);
    let (done, flushed) = mpsc::channel();
    writer.flush(done);
    flushed.recv_timeout(Duration::from_secs(5)).expect("flushed");
    assert_eq!(*written.lock().unwrap(), ["1", "1000"]);
    let answered: Vec<u64> = rx
        .try_iter()
        .filter_map(|ev| match ev {
            AppEvent::FilterHistory(s) => Some(s.seq),
            _ => None,
        })
        .collect();
    assert_eq!(answered, [1, 1000]);
}

/// A clear asked again and again while its write is stuck keeps one thing to say for that
/// write, however often it is asked (the latest): nothing is said before the write is done, and
/// it is said once it is.
#[test]
fn repeated_clears_behind_a_stuck_write_keep_one_thing_to_say() {
    let (gate, written) = (Arc::new(Mutex::new(())), Arc::new(Mutex::new(Vec::new())));
    let held = gate.lock().unwrap();
    let mut app = App::new(&datarig_core::config::Config::default(), None, datarig_i18n::Lang::En);
    let state = std::env::temp_dir().join(format!("datarig-history-announce-{}", std::process::id()));
    app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state) });
    let key = TableKey {
        profile: datarig_core::profile::ProfileId::new(),
        database: "postgres".into(),
        schema: "shop".into(),
        table: "users".into(),
    };
    let mut h = History::default();
    h.record(key.clone(), "id > 1", "", 1);
    app.filter_history.held = Held::Loaded(h);
    app.filter_history.writer = Some(gated(&gate, &written));
    for _ in 0..1000 {
        app.filter_history.clear_asked = Some(key.clone());
        app.filter_history_clear_confirmed(false);
        let waiting = app.filter_history.announce.len();
        assert!(waiting <= 1, "{waiting} things to say for one write");
    }
    let said = |app: &mut App| app.status_line().map(|(m, _, _)| m.to_string()).unwrap_or_default();
    assert!(!said(&mut app).contains("cleared"), "said before the write is done: {}", said(&mut app));
    drop(held);
    app.flush_filter_history();
    assert!(said(&mut app).contains("cleared (0 entries)"), "{}", said(&mut app));
    assert!(app.filter_history.announce.is_empty(), "said once");
    assert_eq!(written.lock().unwrap().len(), 1, "one write");
}
