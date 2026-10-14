//! Counters for the benchmark harness (`datarig-bench`), off unless `DATARIG_BENCH_STATS`
//! names a file: the time from process start to the first frame, frames drawn and event loop
//! wakeups. The file is rewritten with the totals at most once a second while the loop runs
//! (never on a wakeup of its own) and at exit, as one line of JSON.

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) const ENV: &str = "DATARIG_BENCH_STATS";

pub(crate) struct Stats {
    path: PathBuf,
    start: Instant,
    first_frame: Option<Duration>,
    draws: u64,
    wakeups: u64,
    written: Option<Instant>,
}

impl Stats {
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os(ENV).filter(|p| !p.is_empty())?;
        Some(Self {
            path: PathBuf::from(path),
            start: Instant::now(),
            first_frame: None,
            draws: 0,
            wakeups: 0,
            written: None,
        })
    }

    /// A frame was drawn.
    pub(crate) fn drawn(&mut self) {
        self.draws += 1;
        if self.first_frame.is_none() {
            self.first_frame = Some(self.start.elapsed());
            self.write();
        } else if self.written.is_none_or(|w| w.elapsed() >= Duration::from_secs(1)) {
            self.write();
        }
    }

    /// The event loop woke up (an input event, a background event or a timer tick).
    pub(crate) fn woke(&mut self) {
        self.wakeups += 1;
    }

    pub(crate) fn write(&mut self) {
        self.written = Some(Instant::now());
        let line = format!(
            "{{\"first_frame_us\":{},\"draws\":{},\"wakeups\":{},\"uptime_ms\":{}}}\n",
            self.first_frame.map_or(0, |d| d.as_micros()),
            self.draws,
            self.wakeups,
            self.start.elapsed().as_millis()
        );
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, line).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}
