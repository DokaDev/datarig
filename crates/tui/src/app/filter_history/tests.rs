use super::*;
use std::path::Path;

/// A writer whose writes wait for `gate` and are recorded in `written` (their texts).
fn gated(gate: &Arc<Mutex<()>>, written: &Arc<Mutex<Vec<String>>>) -> Writer {
    let (gate, written) = (gate.clone(), written.clone());
    Writer::spawn(Box::new(move |_: &Path, text: &str| {
        let _open = gate.lock().unwrap_or_else(PoisonError::into_inner);
        written.lock().unwrap().push(text.to_string());
        Ok(())
    }))
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
