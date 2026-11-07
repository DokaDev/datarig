//! Counters as rates, on the server's clock, and the bounded history the charts draw.
//!
//! A rate is the counter's change over the change of the server's clock between two samples.
//! Where the two samples are not one run of the counters (a restart, a reset, a database
//! dropped, a new monitor connection), or a counter went down, there is no rate: a [`Gap`], never
//! a negative or a huge spike. The monitor's own traffic is taken out of the counters that
//! count it, so a quiet server reads as quiet.

use super::model::{CounterKind, Sample, Timestamp};
use std::collections::VecDeque;

/// A counter's rate between two samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// So many per second.
    PerSecond(f64),
    /// No rate can be told across these two samples.
    Gap(Gap),
}

/// Why two samples give no rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    /// The counters started again in between (a restart, a statistics reset, a database
    /// dropped).
    Reset,
    /// The counter went down while the run went on (MySQL `FLUSH STATUS` resets some).
    Backwards,
    /// The samples come from two monitor connections, whose own traffic counts differ.
    Reconnected,
    /// The server's clock did not move forward between them.
    NoTime,
}

/// The monitor's own traffic the counters did not show yet. PostgreSQL publishes a session's
/// counts when it flushes its statistics (about once a second while busy, later while idle), so
/// a transaction of the monitor's may show in the counters a sample or more after it was
/// counted in [`super::Own`]. What could not be taken out of one interval is carried to the
/// next ([`rate`]), so the monitor's own transactions never show as load, whenever they are
/// published. Real load drains it at once; it starts again from nothing across a [`Gap`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub statements: u64,
    pub transactions: u64,
}

impl Pending {
    fn of(&mut self, kind: CounterKind) -> Option<&mut u64> {
        match kind {
            CounterKind::Statements => Some(&mut self.statements),
            CounterKind::Commits => Some(&mut self.transactions),
            _ => None,
        }
    }
}

/// The rate of `kind` from `before` to `now`, `None` when the format does not have it.
/// `pending` is the monitor's own traffic not yet shown at `before`, and becomes what is still
/// not shown at `now`: pass the same one for every pair of samples in turn.
pub fn rate(before: &Sample, now: &Sample, kind: CounterKind, pending: &mut Pending) -> Option<Rate> {
    let (a, b) = (before.get(kind)?, now.get(kind)?);
    Some(match delta(before, now, a, b, kind, pending.of(kind)) {
        Ok(d) => match seconds(before.time, now.time) {
            Some(s) => Rate::PerSecond(d as f64 / s),
            None => Rate::Gap(Gap::NoTime),
        },
        Err(gap) => Rate::Gap(gap),
    })
}

/// The share of page requests the page cache answered between two samples, from 0 to 1;
/// `None` when the format does not have the counters or no page was asked for (no answer is
/// not a perfect one).
pub fn cache_hit(before: &Sample, now: &Sample) -> Option<Result<f64, Gap>> {
    let requests = (before.get(CounterKind::PageRequests)?, now.get(CounterKind::PageRequests)?);
    let reads = (before.get(CounterKind::PageReads)?, now.get(CounterKind::PageReads)?);
    let requested = match delta(before, now, requests.0, requests.1, CounterKind::PageRequests, None) {
        Ok(0) => return None,
        Ok(d) => d,
        Err(gap) => return Some(Err(gap)),
    };
    let read = match delta(before, now, reads.0, reads.1, CounterKind::PageReads, None) {
        Ok(d) => d,
        Err(gap) => return Some(Err(gap)),
    };
    Some(Ok(1.0 - (read.min(requested) as f64 / requested as f64)))
}

/// The change of a counter between two samples, the monitor's own traffic (`pending`, for a
/// kind that counts it) taken out.
fn delta(
    before: &Sample,
    now: &Sample,
    a: u64,
    b: u64,
    kind: CounterKind,
    pending: Option<&mut u64>,
) -> Result<u64, Gap> {
    let own = match kind {
        CounterKind::Statements => (before.own.statements, now.own.statements),
        CounterKind::Commits => (before.own.transactions, now.own.transactions),
        _ => (0, 0),
    };
    let d = if !now.epoch.continues(before.epoch) {
        Err(Gap::Reset)
    } else if own.1 < own.0 {
        Err(Gap::Reconnected)
    } else {
        b.checked_sub(a).ok_or(Gap::Backwards)
    };
    match (d, pending) {
        (Err(gap), Some(pending)) => {
            *pending = 0;
            Err(gap)
        }
        (Err(gap), None) => Err(gap),
        (Ok(d), Some(pending)) => {
            let owed = pending.saturating_add(own.1 - own.0);
            *pending = owed.saturating_sub(d);
            Ok(d.saturating_sub(owed))
        }
        (Ok(d), None) => Ok(d),
    }
}

/// Seconds from `a` to `b` on the server's clock, `None` unless it moved forward.
fn seconds(a: Timestamp, b: Timestamp) -> Option<f64> {
    b.since(a).filter(|us| *us > 0).map(|us| us as f64 / 1_000_000.0)
}

/// The recent values of one metric, oldest first, at most `capacity` of them: what a chart
/// draws. A `None` value is a gap (no rate, or no reading while the monitor was not looking),
/// drawn as nothing rather than joined across.
#[derive(Clone, Debug)]
pub struct History {
    capacity: usize,
    points: VecDeque<(Timestamp, Option<f64>)>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        History { capacity: capacity.max(1), points: VecDeque::with_capacity(capacity.max(1)) }
    }

    /// Adds a value read at `time`, dropping the oldest past the capacity. A time not after the
    /// last one's is refused (an answer out of order), so the history stays in clock order.
    pub fn push(&mut self, time: Timestamp, value: Option<f64>) -> bool {
        if self.points.back().is_some_and(|(last, _)| *last >= time) {
            return false;
        }
        if self.points.len() == self.capacity {
            self.points.pop_front();
        }
        self.points.push_back((time, value));
        true
    }

    pub fn points(&self) -> impl ExactSizeIterator<Item = (Timestamp, Option<f64>)> + '_ {
        self.points.iter().copied()
    }

    /// The latest value, `None` when there is none or it is a gap.
    pub fn last(&self) -> Option<f64> {
        self.points.back().and_then(|(_, v)| *v)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}
