//! Paging a result (policy `paging_idle_timeout`): a
//! result with more rows keeps its server-side portal open, and outside the user's transaction
//! the transaction of its own that holds it. When nobody fetches from it for the policy's
//! timeout, the app asks the driver to close it (`DbCommand::ClosePortal`). A portal inside the
//! user's own transaction (`BEGIN`) is never closed for being idle: the user reads uncommitted
//! changes there, and the portal ends with the transaction anyway. The user's transaction is
//! never ended by this: only the portal closes.
//!
//! Pages are explicit: the grid shows one page of the fetched rows at a
//! time; the next page past them is fetched while the portal is open, or, once it closed, by
//! running the statement again when it is on the plain-`SELECT` allowlist
//! (`sql::risk::repeat`), announced; anything else is refused.

use std::time::{Duration, Instant};

/// The paging state of a tab's result (the last statement of its last run).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paging {
    /// No portal is open: no result, a complete one, or one that failed; or its session went
    /// away (lost, rebound, disconnected), after which nothing is fetched for it.
    #[default]
    None,
    /// The result has more rows and its portal is open; `since` is the last fetch activity (the
    /// page that arrived last). `in_block`: the portal lives in the user's own transaction, and
    /// is never closed for being idle.
    Open { since: Instant, in_block: bool },
    /// Closed after the idle timeout; the rows already fetched stay, and the server may have
    /// more.
    ClosedIdle,
    /// Closed because another statement ran in the tab (a run without rows keeps these rows on
    /// screen); the server may have more.
    Replaced,
    /// A fetch of more rows failed or was cancelled: the portal is gone (the driver ended it
    /// with its transaction, or the user's block is aborted), the rows already fetched stay,
    /// and the server may have more (unknown is not absent).
    Interrupted,
    /// Fetching stopped because the result's spill file reached its limit or could not be
    /// written; the rows already shown stay, and the portal was closed.
    Stopped,
}

impl Paging {
    /// Time left before an open portal is closed (`None` when none is open, it lives in the
    /// user's transaction, or the policy never closes it).
    pub fn left(&self, now: Instant, timeout: Option<Duration>) -> Option<Duration> {
        match (self, timeout) {
            (Paging::Open { since, in_block: false }, Some(t)) => {
                Some(t.saturating_sub(now.saturating_duration_since(*since)))
            }
            _ => None,
        }
    }

    /// The open portal has been idle for the whole timeout.
    pub fn due(&self, now: Instant, timeout: Option<Duration>) -> bool {
        self.left(now, timeout).is_some_and(|l| l.is_zero())
    }

    /// The portal was closed while the server may still have rows (idle, another statement
    /// ran, a fetch failed or was cancelled): past the fetched rows the statement can only be
    /// run again.
    pub fn closed(&self) -> bool {
        matches!(self, Paging::ClosedIdle | Paging::Replaced | Paging::Interrupted)
    }
}

/// The page that holds row `row` (from 0), `size` rows a page.
pub fn page_of(row: usize, size: usize) -> usize {
    row / size.max(1)
}

/// How many pages `rows` rows make (at least one).
pub fn pages(rows: u64, size: usize) -> u64 {
    rows.div_ceil(size.max(1) as u64).max(1)
}

/// `23s`: whole seconds, rounded up (it reads `1s` until the very end).
pub fn fmt_left(d: Duration) -> String {
    format!("{}s", d.as_secs() + u64::from(d.subsec_nanos() > 0))
}

#[cfg(test)]
mod tests;
