//! The monitor's replication statements (PostgreSQL 12 on), each giving the columns its
//! decoder in the core names (`monitor::decode::pg::replication::{STATE, SENDERS, RECEIVER,
//! SLOTS}`). The monitor session runs them in one read-only transaction of their own (so a
//! replication read never fails a tick), in one round trip.
//!
//! They read the replication views and the log position functions only: no lock is asked of
//! the lock manager, no user's table is touched, and no connection string (the WAL receiver's,
//! a subscription's: they may hold a user and a host) is ever read.
//! Log positions come back as byte counts, times as microseconds since the Unix epoch.

use super::us;
use datarig_core::monitor::PgMonitor;

/// A log position as the byte count it stands for (NULL stays NULL).
fn bytes(lsn: &str) -> String {
    format!("pg_wal_lsn_diff({lsn}, '0/0')::int8")
}

/// Where the server's log stands: a primary's latest write; a standby's latest received or
/// replayed position, whichever is further (a standby that lost its stream may replay past it
/// from an archive; `GREATEST` skips a NULL). `pg_current_wal_lsn()` fails during recovery, so
/// it is never called there.
const POSITION: &str = "CASE WHEN pg_is_in_recovery() THEN GREATEST(pg_last_wal_receive_lsn(), \
                        pg_last_wal_replay_lsn()) ELSE pg_current_wal_lsn() END";

/// The server's own row. Columns: `now`, `in_recovery`, `position`, `received`, `replayed`,
/// `replayed_at` (when the last replayed transaction committed on the primary), `replay_paused`
/// (NULL on a primary).
pub(crate) fn state() -> String {
    format!(
        "SELECT {now} AS now, pg_is_in_recovery() AS in_recovery, {position} AS position, \
         {received} AS received, {replayed} AS replayed, {replayed_at} AS replayed_at, \
         CASE WHEN pg_is_in_recovery() THEN pg_is_wal_replay_paused() END AS replay_paused",
        now = us("now()"),
        position = bytes(POSITION),
        received = bytes("pg_last_wal_receive_lsn()"),
        replayed = bytes("pg_last_wal_replay_lsn()"),
        replayed_at = us("pg_last_xact_replay_timestamp()"),
    )
}

/// The WAL senders, one row each (ordered by application name, then pid). Columns: `pid`,
/// `application_name`, `client_addr`, `client_port`, `state`, `sync_state`, `sent`,
/// `written`, `flushed`, `replayed`, `write_lag`, `flush_lag`, `replay_lag`, `reply_time`.
/// Without `pg_read_all_stats`, another role's sender shows its pid only.
pub(crate) fn senders() -> String {
    format!(
        "SELECT r.pid, r.application_name, host(r.client_addr) AS client_addr, r.client_port, r.state, \
         r.sync_state, {sent} AS sent, {written} AS written, {flushed} AS flushed, {replayed} AS replayed, \
         {write_lag} AS write_lag, {flush_lag} AS flush_lag, {replay_lag} AS replay_lag, \
         {reply} AS reply_time FROM pg_stat_replication r ORDER BY r.application_name, r.pid",
        sent = bytes("r.sent_lsn"),
        written = bytes("r.write_lsn"),
        flushed = bytes("r.flush_lsn"),
        replayed = bytes("r.replay_lsn"),
        write_lag = us("r.write_lag"),
        flush_lag = us("r.flush_lag"),
        replay_lag = us("r.replay_lag"),
        reply = us("r.reply_time"),
    )
}

/// The WAL receiver, none or one row. Columns: `pid`, `status`, `last_message` (when the
/// primary was last heard from), `slot_name`, `sender_host`, `sender_port`. Without
/// `pg_read_all_stats`, its pid only.
pub(crate) fn receiver() -> String {
    format!(
        "SELECT w.pid, w.status, {last} AS last_message, w.slot_name, w.sender_host, w.sender_port \
         FROM pg_stat_wal_receiver w WHERE w.pid IS NOT NULL",
        last = us("w.last_msg_receipt_time"),
    )
}

/// The replication slots, one row each (ordered by name). Columns: `slot_name`, `slot_type`,
/// `plugin`, `database`, `temporary`, `active`, `active_pid`, `restart`, `wal_status`,
/// `safe_wal_size` (the last two from 13 on, else NULL).
pub(crate) fn slots(pg: &PgMonitor) -> String {
    let status = if pg.version >= 130000 {
        "s.wal_status, s.safe_wal_size"
    } else {
        "NULL AS wal_status, NULL::int8 AS safe_wal_size"
    };
    format!(
        "SELECT s.slot_name::text AS slot_name, s.slot_type, s.plugin::text AS plugin, s.database::text AS database, \
         s.temporary, s.active, s.active_pid, {restart} AS restart, {status} \
         FROM pg_replication_slots s ORDER BY s.slot_name",
        restart = bytes("s.restart_lsn"),
    )
}

/// The logical replication subscriptions, the main apply worker of each (ordered by name).
/// Columns: `subname`, `pid` (NULL: no worker runs), `received`, `last_message`. Tablesync
/// workers (a `relid`) and, from 16, parallel apply workers (a `leader_pid`) are left out. Never
/// reads `pg_subscription.subconninfo`.
pub(crate) fn subscriptions(pg: &PgMonitor) -> String {
    let leader = if pg.version >= 160000 { " AND s.leader_pid IS NULL" } else { "" };
    format!(
        "SELECT s.subname::text AS subname, s.pid, {received} AS received, {last} AS last_message \
         FROM pg_stat_subscription s WHERE s.relid IS NULL{leader} ORDER BY s.subname",
        received = bytes("s.received_lsn"),
        last = us("s.last_msg_receipt_time"),
    )
}

/// The statements of a replication read, in the decoder's order.
pub(crate) fn read(pg: &PgMonitor) -> Vec<String> {
    vec![state(), senders(), receiver(), slots(pg), subscriptions(pg)]
}
