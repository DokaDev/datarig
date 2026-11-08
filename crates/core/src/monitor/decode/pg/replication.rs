//! PostgreSQL replication rows read into the model (the statements are the driver's: its
//! `meta::monitor::replication`).

use super::super::{DecodeError, Row, Rows, malformed, one};
use crate::monitor::model::{Hidden, Need, Part, Visibility};
use crate::monitor::replication::{Lsn, Replica, Replication, Running, Slot, Subscription, Upstream};

/// The columns of the server's own row: whether it is a standby, where its log stands (the
/// primary's latest write, a standby's latest received or replayed position, the further), a standby's
/// latest received and replayed positions, when the last replayed transaction committed on the
/// primary, and whether replay is paused (log positions as byte counts, times as microseconds
/// since the Unix epoch), in order.
pub const STATE: &[&str] = &["now", "in_recovery", "position", "received", "replayed", "replayed_at", "replay_paused"];

/// The columns of a WAL sender's row (`pg_stat_replication`; positions as byte counts, lags in
/// microseconds), in order.
pub const SENDERS: &[&str] = &[
    "pid",
    "application_name",
    "client_addr",
    "client_port",
    "state",
    "sync_state",
    "sent",
    "written",
    "flushed",
    "replayed",
    "write_lag",
    "flush_lag",
    "replay_lag",
    "reply_time",
];

/// The columns of the WAL receiver's row (`pg_stat_wal_receiver`, none when no receiver runs;
/// never its connection string), in order.
pub const RECEIVER: &[&str] = &["pid", "status", "last_message", "slot_name", "sender_host", "sender_port"];

/// The columns of a replication slot's row (`pg_replication_slots`; `restart` as a byte count;
/// `wal_status` and `safe_wal_size` NULL before 13), in order.
pub const SLOTS: &[&str] = &[
    "slot_name",
    "slot_type",
    "plugin",
    "database",
    "temporary",
    "active",
    "active_pid",
    "restart",
    "wal_status",
    "safe_wal_size",
];

/// The columns of a logical replication subscription's row (`pg_stat_subscription`, its main
/// apply worker; `pid` NULL when none runs), in order.
pub const SUBSCRIPTIONS: &[&str] = &["subname", "pid", "received", "last_message"];

/// The rows of a replication read, in the order of its statements: the server's own row
/// ([`STATE`]), the WAL senders ([`SENDERS`]), the WAL receiver ([`RECEIVER`]), the slots
/// ([`SLOTS`]), the subscriptions ([`SUBSCRIPTIONS`]).
pub fn read(rows: &[Rows]) -> Result<Replication, DecodeError> {
    let [state, senders, receiver, slots, subscriptions] = rows else {
        return Err(malformed("replication", format!("{} statements, expected 5", rows.len())));
    };
    let row = one(state, "replication", STATE)?;
    let in_recovery = row.required_bool("in_recovery")?;
    let position = lsn(&row, "position")?;
    let mut hidden_details = false;

    let replicas = senders
        .iter()
        .map(|cells| {
            let r = Row::new(cells, "WAL senders", SENDERS)?;
            // Without `pg_read_all_stats` only the process id of another role's sender shows.
            let visible = if r.text("state").is_some() { Visibility::Full } else { Visibility::Hidden };
            hidden_details |= visible == Visibility::Hidden;
            Ok(Replica {
                id: r.int("pid")?,
                name: r.string("application_name").filter(|n| !n.is_empty()),
                client: address(&r, "client_addr", "client_port")?,
                state: r.string("state"),
                sync: r.string("sync_state"),
                sent: lsn(&r, "sent")?,
                written: lsn(&r, "written")?,
                flushed: lsn(&r, "flushed")?,
                replayed: lsn(&r, "replayed")?,
                write_lag: r.int("write_lag")?,
                flush_lag: r.int("flush_lag")?,
                replay_lag: r.int("replay_lag")?,
                reply: r.time("reply_time")?,
                user: None,
                visible,
            })
        })
        .collect::<Result<Vec<_>, DecodeError>>()?;

    let upstream = if in_recovery {
        let receiver = match receiver.as_slice() {
            [] => None,
            [cells] => Some(Row::new(cells, "WAL receiver", RECEIVER)?),
            _ => return Err(malformed("WAL receiver", format!("{} rows, expected at most 1", receiver.len()))),
        };
        let applier = match row.bool("replay_paused")? {
            Some(true) => Running::Paused,
            _ => Running::Yes,
        };
        let mut upstream = Upstream {
            channel: None,
            source: None,
            slot: None,
            // No receiver: the standby replays what it has, or reads from an archive.
            receiver: Running::No,
            receiver_state: None,
            applier,
            applier_state: None,
            // Read by functions every account may call: known whatever the receiver shows.
            received: lsn(&row, "received")?,
            applied: lsn(&row, "replayed")?,
            received_set: None,
            executed_set: None,
            gtids_only: false,
            gtids_left: None,
            source_read: None,
            source_applied: None,
            applier_behind: None,
            delay: None,
            last_message: None,
            last_received: None,
            last_applied: row.time("replayed_at")?,
            errors: Vec::new(),
            visible: Visibility::Full,
        };
        if let Some(r) = receiver {
            match r.string("status") {
                Some(status) => {
                    upstream.receiver = if status == "streaming" { Running::Yes } else { Running::Other(status) };
                    upstream.source = address(&r, "sender_host", "sender_port")?;
                    upstream.slot = r.string("slot_name");
                    upstream.last_message = r.time("last_message")?;
                }
                None => {
                    upstream.receiver = Running::Unknown;
                    upstream.visible = Visibility::Hidden;
                    hidden_details = true;
                }
            }
        }
        vec![upstream]
    } else {
        Vec::new()
    };

    let slots = slots
        .iter()
        .map(|cells| {
            let r = Row::new(cells, "replication slots", SLOTS)?;
            let restart = lsn(&r, "restart")?;
            Ok(Slot {
                name: r.required("slot_name")?.to_owned(),
                kind: r.required("slot_type")?.to_owned(),
                plugin: r.string("plugin"),
                database: r.string("database"),
                temporary: r.required_bool("temporary")?,
                active: r.required_bool("active")?,
                active_pid: r.int("active_pid")?,
                restart,
                kept: restart.zip(position).and_then(|(restart, position)| restart.bytes_to(position)),
                wal_status: r.string("wal_status"),
                safe_wal_size: r.int("safe_wal_size")?,
            })
        })
        .collect::<Result<Vec<_>, DecodeError>>()?;

    let subscriptions = subscriptions
        .iter()
        .map(|cells| {
            let r = Row::new(cells, "subscriptions", SUBSCRIPTIONS)?;
            Ok(Subscription {
                name: r.required("subname")?.to_owned(),
                running: r.int("pid")?.is_some(),
                received: lsn(&r, "received")?,
                last_message: r.time("last_message")?,
            })
        })
        .collect::<Result<Vec<_>, DecodeError>>()?;

    let hidden = if hidden_details {
        vec![Hidden { part: Part::ReplicationDetails, need: Need::Grant("pg_read_all_stats") }]
    } else {
        Vec::new()
    };
    Ok(Replication {
        time: row.required_time("now")?,
        in_recovery: Some(in_recovery),
        position,
        upstream: Some(upstream),
        replicas: Some(replicas),
        slots: Some(slots),
        subscriptions: Some(subscriptions),
        members: Vec::new(),
        hidden,
    })
}

/// A log position given as a byte count.
fn lsn(row: &Row, name: &str) -> Result<Option<Lsn>, DecodeError> {
    Ok(row.uint(name)?.map(Lsn))
}

/// An address, with its port where there is one (a local socket has port -1 or none).
fn address(row: &Row, host: &str, port: &str) -> Result<Option<String>, DecodeError> {
    Ok(match (row.string(host), row.int(port)?) {
        (Some(host), Some(port)) if port > 0 => {
            Some(if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") })
        }
        (host, _) => host,
    })
}
