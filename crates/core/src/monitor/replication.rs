//! Replication as the server tells it: whether it takes changes from another server, which
//! servers take changes from it, and (PostgreSQL) the slots that keep changes for them.
//!
//! A lag is said only as far as the server can tell it, and never as a figure that hides what
//! it does not measure: PostgreSQL's last replayed transaction ages on a primary with no writes
//! too, and MySQL's `Seconds_Behind_Source` measures the applier against what the receiver
//! fetched, so a receiver that is behind or stalled shows 0 there. [`Upstream::lag`] and
//! [`Replica::lag`] say what is known: applied all it received, how much it has not applied
//! yet, stopped, or not known.

use super::model::{Hidden, Part, Timestamp, Visibility};
use std::fmt;

/// A position in PostgreSQL's write-ahead log (an LSN), as the byte count it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lsn(pub u64);

impl Lsn {
    /// The bytes from this position to `later`, `None` when `later` is before it.
    pub fn bytes_to(self, later: Lsn) -> Option<u64> {
        later.0.checked_sub(self.0)
    }
}

impl fmt::Display for Lsn {
    /// As the server writes it: `16/B374D848`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:X}/{:X}", self.0 >> 32, self.0 & 0xFFFF_FFFF)
    }
}

/// What a replication read found ([`crate::driver::DbCommand::MonitorReplication`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replication {
    /// The server's clock when it was read: every age is worked out on it.
    pub time: Timestamp,
    /// PostgreSQL: the server is a standby (in recovery). `None` for MySQL, which tells it by
    /// its channels ([`Replication::upstream`]).
    pub in_recovery: Option<bool>,
    /// PostgreSQL: where the server's own log stands (the primary's latest write, a standby's
    /// latest received or replayed position): what a slot's kept log is measured to.
    pub position: Option<Lsn>,
    /// What this server takes changes from: a PostgreSQL standby's WAL receiver, MySQL's
    /// channels. `None`: the account may not read it ([`Replication::hidden`] says what would
    /// show it).
    pub upstream: Option<Vec<Upstream>>,
    /// The servers that take changes from this one. `None`: the account may not read it.
    pub replicas: Option<Vec<Replica>>,
    /// PostgreSQL replication slots: the log each keeps for its consumer. `None` for MySQL,
    /// which has none.
    pub slots: Option<Vec<Slot>>,
    /// PostgreSQL logical replication subscriptions of this server (it takes changes of some
    /// tables from another server, while it takes writes itself). `None` for MySQL.
    pub subscriptions: Option<Vec<Subscription>>,
    /// MySQL group replication members, empty when the server is in no group (or the account
    /// may not read them: then [`Replication::hidden`] says so).
    pub members: Vec<Member>,
    /// What the account could not read, and what would show it.
    pub hidden: Vec<Hidden>,
}

/// What a server is in replication, as far as the read tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// It takes changes from another server (it may pass them on too: [`Replication::replicas`]).
    Replica,
    /// Other servers take changes from it and it takes none.
    Primary,
    /// It takes no changes and none are taken from it now (slots may still keep log for a
    /// consumer that is away).
    Standalone,
    /// The account may not read what would tell it.
    Unknown,
}

impl Replication {
    /// What the server is: a replica when it takes changes (a channel, recovery, a secondary
    /// of its replication group), else a primary when servers take changes from it (or it is
    /// its group's primary); not known when a read that would tell was refused: a server with
    /// replicas whose own source could not be read may take changes too.
    pub fn role(&self) -> Role {
        match self.members.iter().find(|m| m.this && m.state == "ONLINE").and_then(|m| m.role.as_deref()) {
            Some("PRIMARY") => return Role::Primary,
            Some("SECONDARY") => return Role::Replica,
            _ => {}
        }
        let subscribed = self.subscriptions.as_ref().is_some_and(|s| !s.is_empty());
        let replica = match (self.in_recovery, &self.upstream) {
            (Some(recovery), _) => Some(recovery || subscribed),
            (None, Some(channels)) => Some(!channels.is_empty()),
            (None, None) => None,
        };
        let primary = self.replicas.as_ref().map(|r| !r.is_empty());
        let group_unknown = self.hidden.iter().any(|h| h.part == Part::GroupMembers);
        match (replica, primary) {
            (Some(true), _) => Role::Replica,
            // A group secondary takes changes by the group's applier, which no channel lists.
            (Some(false), Some(true)) if !group_unknown => Role::Primary,
            (Some(false), Some(false)) if !group_unknown => Role::Standalone,
            _ => Role::Unknown,
        }
    }
}

/// A place in a MySQL source's binary log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogPos {
    pub file: String,
    pub pos: u64,
}

impl std::fmt::Display for LogPos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.pos)
    }
}

/// Whether a part of replication runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Running {
    Yes,
    No,
    /// Trying to reach its source (MySQL `Connecting`).
    Connecting,
    /// Paused on request (PostgreSQL `pg_wal_replay_pause()`).
    Paused,
    /// Another state, in the server's words (PostgreSQL WAL receiver `starting`, `waiting`,
    /// `stopping`).
    Other(String),
    /// The account may not see it.
    Unknown,
}

/// The side of a channel an error belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// What fetches the changes (MySQL I/O thread).
    Receiver,
    /// What applies them (MySQL SQL thread).
    Applier,
}

/// The last error a side of a channel reported (MySQL `Last_IO_Error`, `Last_SQL_Error`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelError {
    pub side: Side,
    pub code: u32,
    pub message: String,
    /// When, in the server's words (MySQL `Last_IO_Error_Timestamp`: its local time).
    pub at: Option<String>,
}

/// What this server takes changes from: a PostgreSQL standby's WAL receiver, or one MySQL
/// replication channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// MySQL channel name, `None` for the default channel (and PostgreSQL).
    pub channel: Option<String>,
    /// The source's host and port, as the server says (PostgreSQL `sender_host`, MySQL
    /// `Source_Host`); never the connection string, which may hold a user.
    pub source: Option<String>,
    /// PostgreSQL: the slot the receiver streams from.
    pub slot: Option<String>,
    /// What fetches the changes: PostgreSQL's WAL receiver (`No`: there is none, so the standby
    /// replays only what it has or reads from an archive), MySQL's I/O thread.
    pub receiver: Running,
    /// What the receiver is doing, in the server's words (MySQL `Replica_IO_State`).
    pub receiver_state: Option<String>,
    /// What applies them: PostgreSQL replay (`Paused` when paused), MySQL's SQL thread.
    pub applier: Running,
    /// What the applier is doing, in the server's words (MySQL `Replica_SQL_Running_State`).
    pub applier_state: Option<String>,
    /// PostgreSQL: the last log position received and the last replayed.
    pub received: Option<Lsn>,
    pub applied: Option<Lsn>,
    /// MySQL: the GTID sets received and executed (`Retrieved_Gtid_Set`,
    /// `Executed_Gtid_Set`), as the server writes them.
    pub received_set: Option<String>,
    pub executed_set: Option<String>,
    /// MySQL: every transaction carries a GTID (`gtid_mode` `ON`), so the sets account for all
    /// that was received; else (anonymous transactions possible) they do not tell a lag.
    pub gtids_only: bool,
    /// The transactions of [`Upstream::received_set`] not in [`Upstream::executed_set`], where
    /// the sets count them ([`Upstream::count_gtids`]: worked out once per read, never per
    /// frame).
    pub gtids_left: Option<u64>,
    /// MySQL: where in the source's binary log the receiver read to (`Source_Log_File`,
    /// `Read_Source_Log_Pos`) and the applier applied to (`Relay_Source_Log_File`,
    /// `Exec_Source_Log_Pos`).
    pub source_read: Option<LogPos>,
    pub source_applied: Option<LogPos>,
    /// MySQL `Seconds_Behind_Source`: how far the applier is behind the source's time of the
    /// transactions the receiver fetched (so 0 while the receiver itself lags). `None` when the
    /// server says NULL (a thread is not running).
    pub applier_behind: Option<u64>,
    /// MySQL `SQL_Delay`: a delay asked for, in seconds (0: none).
    pub delay: Option<u64>,
    /// When the source last answered: PostgreSQL `last_msg_receipt_time`, MySQL
    /// `LAST_HEARTBEAT_TIMESTAMP` (a source sends a heartbeat only while it has nothing else to
    /// send).
    pub last_message: Option<Timestamp>,
    /// MySQL: when the receiver last finished fetching a transaction
    /// (`LAST_QUEUED_TRANSACTION_END_QUEUE_TIMESTAMP`).
    pub last_received: Option<Timestamp>,
    /// When the last transaction applied was committed on its source (PostgreSQL
    /// `pg_last_xact_replay_timestamp()`, on the primary's clock) or (MySQL
    /// `LAST_APPLIED_TRANSACTION_END_APPLY_TIMESTAMP`) applied here. Ages on a source with no
    /// writes: never a lag on its own.
    pub last_applied: Option<Timestamp>,
    pub errors: Vec<ChannelError>,
    /// Whether the account sees its state and positions.
    pub visible: Visibility,
}

/// How far a server is behind what it should have applied, said only as far as the server
/// can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lag {
    /// It applied all it received (whether its receiver is behind its source is another
    /// matter: [`Upstream::receiver`] and [`Upstream::last_message`] tell).
    CaughtUp,
    /// It has not applied all it received: PostgreSQL the bytes of log not replayed yet; MySQL
    /// the transactions received but not executed (from the GTID sets, where both are read)
    /// and `Seconds_Behind_Source`.
    Behind { bytes: Option<u64>, transactions: Option<u64>, seconds: Option<u64> },
    /// The applier does not run (stopped, paused or failed), and how much it has left is not
    /// known.
    Stopped,
    /// Not known.
    Unknown,
}

impl Upstream {
    /// Work out [`Upstream::gtids_left`] from the sets, where they count what is left: every
    /// transaction carries a GTID ([`Upstream::gtids_only`]) and something was received.
    pub fn count_gtids(&mut self) {
        self.gtids_left = match (&self.received_set, &self.executed_set) {
            (Some(received), Some(executed)) if self.gtids_only && !received.trim().is_empty() => {
                gtids_missing(received, executed)
            }
            _ => None,
        };
    }

    /// How far this server is behind what it received (whether its applier runs is
    /// [`Upstream::applier`]: a stopped or paused one is behind by what it has not applied).
    pub fn lag(&self) -> Lag {
        if self.visible == Visibility::Hidden && self.received.is_none() && self.received_set.is_none() {
            return Lag::Unknown;
        }
        let lag = self.measured();
        match (&lag, &self.applier) {
            (Lag::Unknown, Running::No | Running::Paused) => Lag::Stopped,
            _ => lag,
        }
    }

    fn measured(&self) -> Lag {
        if let (Some(received), Some(applied)) = (self.received, self.applied) {
            return match applied.bytes_to(received) {
                Some(0) | None => Lag::CaughtUp,
                Some(bytes) => Lag::Behind { bytes: Some(bytes), transactions: None, seconds: None },
            };
        }
        // Neither log positions nor a channel's sets (a standby that never received any log):
        // nothing to tell by.
        if self.received_set.is_none() && self.executed_set.is_none() {
            return Lag::Unknown;
        }
        // The GTID sets count what is left only where every transaction carries a GTID
        // (`gtid_mode` `ON`): without GTIDs both sets are empty, and in the modes between an
        // anonymous transaction waiting is in neither. Even then a channel may take anonymous
        // transactions and give them GTIDs as it applies them, so the sets never prove that
        // nothing is left.
        let left = self.gtids_left.filter(|n| *n > 0);
        // Where the receiver read the source's log to and where the applier applied it to: the
        // same place proves nothing is left; else the bytes between them, in one file.
        if let (Some(read), Some(applied)) = (&self.source_read, &self.source_applied) {
            if read == applied {
                return Lag::CaughtUp;
            }
            let bytes = (read.file == applied.file).then(|| read.pos.saturating_sub(applied.pos)).filter(|b| *b > 0);
            return Lag::Behind { bytes, transactions: left, seconds: self.applier_behind };
        }
        // Without the positions: `Seconds_Behind_Source` is 0 also with events of the same
        // second not applied yet, so 0 proves nothing; NULL while the applier runs means it has
        // applied all the receiver fetched (and the receiver stopped).
        match (&self.applier, self.applier_behind) {
            (Running::Yes, None) if left.is_none() => Lag::CaughtUp,
            (_, seconds) if left.is_some() || seconds.is_some_and(|s| s > 0) => {
                Lag::Behind { bytes: None, transactions: left, seconds }
            }
            (Running::No | Running::Paused, _) => Lag::Stopped,
            _ => Lag::Unknown,
        }
    }
}

/// A server that takes changes from this one, as this one sees it: a PostgreSQL WAL sender's
/// standby, or a MySQL replica's connection to its source (a binary log dump thread).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replica {
    /// The process id of the WAL sender (PostgreSQL) or the dump thread's connection id
    /// (MySQL).
    pub id: Option<i64>,
    /// PostgreSQL `application_name` (the standby's `cluster_name` or what its connection
    /// asked for).
    pub name: Option<String>,
    /// The replica's address (and port) as this server sees it.
    pub client: Option<String>,
    /// What the sender does, in the server's words: PostgreSQL `startup`, `catchup`,
    /// `streaming`, `backup`, `stopping`; MySQL the dump thread's `STATE`.
    pub state: Option<String>,
    /// PostgreSQL `sync_state`: `async`, `potential`, `sync`, `quorum`.
    pub sync: Option<String>,
    /// PostgreSQL: the last log position sent, and the last the standby reported written,
    /// flushed and replayed.
    pub sent: Option<Lsn>,
    pub written: Option<Lsn>,
    pub flushed: Option<Lsn>,
    pub replayed: Option<Lsn>,
    /// PostgreSQL `write_lag`, `flush_lag`, `replay_lag` in microseconds: measured only while
    /// there are writes to measure (NULL soon after the standby caught up).
    pub write_lag: Option<i64>,
    pub flush_lag: Option<i64>,
    pub replay_lag: Option<i64>,
    /// PostgreSQL `reply_time`: when the standby last reported, on its clock.
    pub reply: Option<Timestamp>,
    /// MySQL: the account the replica connects with.
    pub user: Option<String>,
    pub visible: Visibility,
}

impl Replica {
    /// How far the replica is behind `position` (this server's latest write): the bytes it has
    /// not replayed yet, or (MySQL, where the source does not know) not known.
    pub fn lag(&self, position: Option<Lsn>) -> Lag {
        if self.visible == Visibility::Hidden {
            return Lag::Unknown;
        }
        match (self.replayed, position) {
            (Some(replayed), Some(position)) => match replayed.bytes_to(position) {
                Some(0) | None => Lag::CaughtUp,
                Some(bytes) => Lag::Behind { bytes: Some(bytes), transactions: None, seconds: None },
            },
            _ => Lag::Unknown,
        }
    }
}

/// A PostgreSQL replication slot: it keeps the log its consumer has not confirmed, whether the
/// consumer is connected or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub name: String,
    /// `physical` or `logical`.
    pub kind: String,
    /// A logical slot's output plugin and database.
    pub plugin: Option<String>,
    pub database: Option<String>,
    pub temporary: bool,
    /// A consumer is connected now (its process id, where the server says).
    pub active: bool,
    pub active_pid: Option<i64>,
    /// The oldest log position the slot keeps.
    pub restart: Option<Lsn>,
    /// The bytes of log the slot keeps: from [`Slot::restart`] to where the server's log stands
    /// ([`Replication::position`]). `None`: the slot keeps none (no restart position) or the
    /// position is not known.
    pub kept: Option<u64>,
    /// `wal_status` (13 on): `reserved`, `extended`, `unreserved` (past what the server keeps:
    /// at risk), `lost` (the consumer cannot continue).
    pub wal_status: Option<String>,
    /// `safe_wal_size` (13 on): bytes that may still be written before the slot is at risk of
    /// losing log (`None`: no limit set, or the server does not say).
    pub safe_wal_size: Option<i64>,
}

/// A PostgreSQL logical replication subscription: its main apply worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    pub name: String,
    /// Its apply worker runs (`pid` not NULL; a disabled or failing subscription has none).
    pub running: bool,
    /// The last log position received from the publisher.
    pub received: Option<Lsn>,
    /// When the publisher was last heard from.
    pub last_message: Option<Timestamp>,
}

/// A member of a MySQL group replication group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// `MEMBER_HOST:MEMBER_PORT`.
    pub address: String,
    /// `MEMBER_STATE`: `ONLINE`, `RECOVERING`, `OFFLINE`, `ERROR`, `UNREACHABLE`.
    pub state: String,
    /// `MEMBER_ROLE`: `PRIMARY` or `SECONDARY`.
    pub role: Option<String>,
    /// The reading server itself.
    pub this: bool,
}

/// The transactions of GTID set `received` that are not in `executed` (MySQL sets as the server
/// writes them: `uuid:1-5:7,uuid2[:tag]:1-3`, with any whitespace). `None` when a set does not
/// read as one.
pub fn gtids_missing(received: &str, executed: &str) -> Option<u64> {
    let received = gtid_set(received)?;
    let executed = gtid_set(executed)?;
    let mut missing = 0u64;
    for ((source, tag), ranges) in &received {
        let done: &[(u64, u64)] = executed.iter().find(|(k, _)| k.0 == *source && k.1 == *tag).map_or(&[], |(_, r)| r);
        // Both lists are sorted and merged: one pass over each.
        let mut j = 0;
        for &(from, to) in ranges {
            let mut count = to - from + 1;
            while j < done.len() && done[j].1 < from {
                j += 1;
            }
            let mut k = j;
            while k < done.len() && done[k].0 <= to {
                let (lo, hi) = (from.max(done[k].0), to.min(done[k].1));
                count -= hi - lo + 1;
                k += 1;
            }
            missing += count;
        }
    }
    Some(missing)
}

/// A GTID set by (source UUID, tag), each with its intervals merged and in order.
type GtidSet = Vec<((String, String), Vec<(u64, u64)>)>;

fn gtid_set(text: &str) -> Option<GtidSet> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut set: GtidSet = Vec::new();
    for part in text.split(',').filter(|p| !p.is_empty()) {
        let mut pieces = part.split(':');
        let source = pieces.next()?.to_ascii_lowercase();
        if source.len() != 36 {
            return None;
        }
        let mut tag = String::new();
        for piece in pieces {
            let range = match piece.split_once('-') {
                Some((a, b)) => (a.parse::<u64>().ok(), b.parse::<u64>().ok()),
                None => (piece.parse::<u64>().ok(), piece.parse::<u64>().ok()),
            };
            match range {
                (Some(from), Some(to)) if from >= 1 && from <= to => {
                    let key = (source.clone(), tag.clone());
                    match set.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, ranges)) => ranges.push((from, to)),
                        None => set.push((key, vec![(from, to)])),
                    }
                }
                // A tag (8.3 on): letters, digits and underscores, starting with a letter or `_`.
                _ if piece.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && piece.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
                {
                    tag = piece.to_ascii_lowercase();
                }
                _ => return None,
            }
        }
    }
    for (_, ranges) in &mut set {
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
        for &(a, b) in ranges.iter() {
            match merged.last_mut() {
                Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        *ranges = merged;
    }
    Some(set)
}
