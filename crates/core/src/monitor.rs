//! Server monitoring: what a server says about its sessions and its load, read by a monitor
//! connection of its own, as one model for every database.
//!
//! Everything here is pure and engine-neutral. The statements of a tick are each driver's (next
//! to its catalog statements, in its `meta::monitor`); the driver runs them on its monitor
//! connection and hands the rows back as text cells, and [`decode`] turns them into the model.
//! Each decoder names the columns it expects ([`decode::pg::SESSIONS`] and the like), and the
//! drivers' tests check their statements give exactly those, so the two never drift apart.
//! Nothing in this module talks to a server, so every format is tested on rows recorded from
//! real servers (`monitor/fixtures`).
//!
//! * [`format`]: what a server offers ([`MonitorFormat`]), learnt from a probe after connecting:
//!   the version gates, what the account may read, and the flavour (self-managed or a managed
//!   service), which is read from the server and never guessed from its host name.
//! * The model ([`ServerInfo`], [`Sample`], [`Gauge`], [`SessionRow`], [`Hidden`]): what a tick
//!   read, with what the account cannot see said as such (unknown is never shown as absent).
//! * [`locks`]: who waits for whom, and the blocking tree.
//! * [`rate`]: counters as rates per second on the server's clock, with resets and the monitor's
//!   own traffic taken out, and the bounded history the charts draw.
//! * [`action`]: cancelling another session's statement or ending it: what may be offered,
//!   what the command carries, what each provider needs, and what a later read confirms.
//!
//! Each format reports the metrics it really has ([`CounterKind`], [`GaugeKind`]); nothing is
//! renamed to look like another engine's (PostgreSQL commits are not MySQL statements).

pub mod action;
pub mod decode;
pub mod format;
pub mod locks;
mod model;
pub mod rate;

pub use action::{
    ActionKind, ActionOutcome, ActionTarget, Ask, Confirmation, Decision, FollowUp, Refusal, StatementStamp, Warning,
};
pub use format::{Flavour, MonitorFormat, MySqlMonitor, PgMonitor, Probe, Readable, Route};
pub use locks::{Blame, Edge, Holder, LockHolder, LockList, LockObject, LockWait, Prepared, Tree, Waits};
pub use model::{
    Blocker, Counter, CounterKind, Epoch, Extra, ExtraKey, FullText, Gauge, GaugeKind, Hidden, MAX_WAITERS, Need, Own,
    Part, Proof, QUERY_CHARS, QueryText, Sample, ServerInfo, SessionKey, SessionKind, SessionList, SessionRow,
    SessionState, SessionsWanted, Snapshot, TEXT_MIN_AGE_US, TEXT_SAME_START_US, TextAsk, Timestamp, Tx, Visibility,
    Wait, WaitClass, Want,
};

#[cfg(test)]
mod tests;
