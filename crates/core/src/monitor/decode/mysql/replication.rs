//! MySQL replication rows read into the model (the statements are the driver's: its
//! `meta::monitor::replication`). Each read may be refused for lack of a privilege: a refusal
//! is said as such ([`Hidden`]), never read as "not replicating".

use super::super::{Answer, DecodeError, Row, Rows, malformed, name_values, one};
use super::REFUSED;
use crate::monitor::format::{MySqlMonitor, Readable};
use crate::monitor::model::{Hidden, Need, Part, Timestamp, Visibility};
use crate::monitor::replication::{ChannelError, LogPos, Member, Replica, Replication, Running, Side, Upstream};

/// The columns of a channel's row of `SHOW REPLICA STATUS` (8.0.22 on) that are read, in order.
/// The statement gives many more: the driver keeps these, by name.
pub const REPLICA_STATUS: &[&str] = &[
    "Channel_Name",
    "Source_Host",
    "Source_Port",
    "Replica_IO_Running",
    "Replica_IO_State",
    "Replica_SQL_Running",
    "Replica_SQL_Running_State",
    "Seconds_Behind_Source",
    "SQL_Delay",
    "Last_IO_Errno",
    "Last_IO_Error",
    "Last_IO_Error_Timestamp",
    "Last_SQL_Errno",
    "Last_SQL_Error",
    "Last_SQL_Error_Timestamp",
    "Retrieved_Gtid_Set",
    "Executed_Gtid_Set",
    "Source_Log_File",
    "Read_Source_Log_Pos",
    "Relay_Source_Log_File",
    "Exec_Source_Log_Pos",
];

/// The same columns as `SHOW SLAVE STATUS` (before 8.0.22) names them, in the order of
/// [`REPLICA_STATUS`].
pub const SLAVE_STATUS: &[&str] = &[
    "Channel_Name",
    "Master_Host",
    "Master_Port",
    "Slave_IO_Running",
    "Slave_IO_State",
    "Slave_SQL_Running",
    "Slave_SQL_Running_State",
    "Seconds_Behind_Master",
    "SQL_Delay",
    "Last_IO_Errno",
    "Last_IO_Error",
    "Last_IO_Error_Timestamp",
    "Last_SQL_Errno",
    "Last_SQL_Error",
    "Last_SQL_Error_Timestamp",
    "Retrieved_Gtid_Set",
    "Executed_Gtid_Set",
    "Master_Log_File",
    "Read_Master_Log_Pos",
    "Relay_Master_Log_File",
    "Exec_Master_Log_Pos",
];

/// The columns of the replication read's first row: the server's clock and `@@gtid_mode` (only
/// `ON` means every transaction carries a GTID, so that the GTID sets can count what a replica
/// received), in order.
pub const STATE: &[&str] = &["now", "gtid_mode"];

/// The status variable that says the performance schema could not track every thread: where
/// the replicas are read from `performance_schema.processlist`, a list it could not track whole
/// may miss one.
pub const THREADS_LOST: &str = "Performance_schema_thread_instances_lost";

/// The columns of a channel's times (`performance_schema.replication_connection_status` and,
/// the latest of its workers, `replication_applier_status_by_worker`; microseconds since the
/// Unix epoch, 0 for never), in order.
pub const CHANNEL_TIMES: &[&str] = &["CHANNEL_NAME", "last_heartbeat", "last_queued", "last_applied"];

/// The columns of a replica's connection to this server (a binary log dump thread of the
/// session list: `SHOW REPLICAS` is not read, as it gives the replicas' passwords where
/// `show_replica_auth_info` is on), in order.
pub const REPLICAS: &[&str] = &["ID", "USER", "HOST", "STATE"];

/// The columns of a group replication member's row (`performance_schema.
/// replication_group_members`; `this`: the reading server), in order.
pub const GROUP_MEMBERS: &[&str] = &["address", "MEMBER_STATE", "MEMBER_ROLE", "this"];

/// The reads of a replication read, each answered or refused on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    /// The server's clock and what tells how far the other reads go ([`STATE`]).
    Clock,
    /// This server's channels ([`REPLICA_STATUS`], needs `REPLICATION CLIENT`).
    Channels,
    /// When each channel last heard from its source and applied a transaction
    /// ([`CHANNEL_TIMES`], needs `SELECT` on the `performance_schema` tables, which are filled
    /// with the performance schema off too).
    ChannelTimes,
    /// [`THREADS_LOST`] (`Variable_name`, `Value`), read where the replicas come from
    /// `performance_schema.processlist`.
    ThreadsLost,
    /// The replicas connected to this server ([`REPLICAS`]: every account's sessions need
    /// `PROCESS`, else only its own are listed).
    Replicas,
    /// The members of the server's replication group ([`GROUP_MEMBERS`], needs `SELECT` on
    /// the `performance_schema` table, filled with the performance schema off too).
    GroupMembers,
}

/// The answers of a replication read, by read; a read missing was not sent (the performance
/// schema is off). `my`: what the probe found (whether the account sees every session).
pub fn read(my: &MySqlMonitor, answers: &[(Read, Answer)]) -> Result<Replication, DecodeError> {
    let answer = |read: Read| answers.iter().find(|(r, _)| *r == read).map(|(_, a)| a);
    let mut hidden = Vec::new();
    let (time, gtids_only) = match answer(Read::Clock) {
        Some(Answer::Rows(rows)) => {
            let row = one(rows, "replication state", STATE)?;
            (row.required_time("now")?, row.text("gtid_mode") == Some("ON"))
        }
        Some(Answer::Error { message, .. }) => return Err(DecodeError::Failed(message.clone())),
        None => return Err(malformed("replication", "no clock")),
    };
    // Not read (the list does not come from the performance schema): nothing lost. Refused:
    // whether the list is whole is not known.
    let threads_lost = match answer(Read::ThreadsLost) {
        None => false,
        some => match rows_of(some)? {
            Some(rows) => name_values(rows, "threads lost")?
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case(THREADS_LOST) && value.is_some_and(|v| v != "0")),
            None => true,
        },
    };

    let mut upstream = match rows_of(answer(Read::Channels))? {
        Some(rows) => {
            Some(rows.iter().map(|cells| channel(cells, gtids_only)).collect::<Result<Vec<_>, DecodeError>>()?)
        }
        None => {
            hidden.push(Hidden { part: Part::ReplicationSources, need: Need::Grant("REPLICATION CLIENT") });
            None
        }
    };
    if let Some(channels) = upstream.as_mut().filter(|c| !c.is_empty()) {
        match answer(Read::ChannelTimes) {
            Some(Answer::Rows(rows)) => {
                for cells in rows {
                    let r = Row::new(cells, "channel times", CHANNEL_TIMES)?;
                    let name = r.string("CHANNEL_NAME").filter(|n| !n.is_empty());
                    if let Some(c) = channels.iter_mut().find(|c| c.channel == name) {
                        c.last_message = moment(&r, "last_heartbeat")?;
                        c.last_received = moment(&r, "last_queued")?;
                        c.last_applied = moment(&r, "last_applied")?;
                    }
                }
            }
            refused @ Some(Answer::Error { .. }) => {
                rows_of(refused)?;
                hidden.push(Hidden { part: Part::ReplicationTimes, need: Need::Grant("SELECT on performance_schema") });
            }
            None => hidden.push(Hidden { part: Part::ReplicationTimes, need: Need::Setting("performance_schema") }),
        }
    }

    // Without `PROCESS` (by a direct grant: a role's is not seen) only the account's own
    // sessions are listed, so no replica listed says nothing, and one listed may not be all.
    // The performance schema could not track every thread: a dump thread may be missing.
    let replicas = match rows_of(answer(Read::Replicas))? {
        Some(rows) if my.process == Readable::Yes && !threads_lost => {
            Some(rows.iter().map(|cells| replica(cells)).collect::<Result<Vec<_>, DecodeError>>()?)
        }
        rows => {
            // What would show the list whole: the table's own grant where the read was refused,
            // else `PROCESS`, else threads the performance schema could track.
            let need = match (&rows, my.process) {
                (None, _) if my.ps_processlist() => Need::Grant("SELECT on performance_schema"),
                (None, _) | (Some(_), Readable::Refused | Readable::Unknown) => Need::Grant("PROCESS"),
                (Some(_), Readable::Yes) => Need::Setting("performance_schema_max_thread_instances"),
            };
            hidden.push(Hidden { part: Part::Replicas, need });
            match rows.filter(|r| !r.is_empty()) {
                Some(rows) => Some(rows.iter().map(|cells| replica(cells)).collect::<Result<Vec<_>, DecodeError>>()?),
                None => None,
            }
        }
    };

    let members = match answer(Read::GroupMembers) {
        Some(Answer::Rows(rows)) => rows
            .iter()
            .map(|cells| {
                let r = Row::new(cells, "group members", GROUP_MEMBERS)?;
                Ok(Member {
                    address: r.required("address")?.to_owned(),
                    state: r.required("MEMBER_STATE")?.to_owned(),
                    role: r.string("MEMBER_ROLE").filter(|r| !r.is_empty()),
                    this: r.required_bool("this")?,
                })
            })
            .collect::<Result<Vec<_>, DecodeError>>()?,
        refused @ Some(Answer::Error { .. }) => {
            rows_of(refused)?;
            hidden.push(Hidden { part: Part::GroupMembers, need: Need::Grant("SELECT on performance_schema") });
            Vec::new()
        }
        None => Vec::new(),
    };

    Ok(Replication {
        time,
        in_recovery: None,
        position: None,
        upstream,
        replicas,
        slots: None,
        subscriptions: None,
        members,
        hidden,
    })
}

/// The rows of an answer, `None` when the server refused it for lack of a privilege. Another
/// failure fails the read: what it would have shown is not known, and not "none".
fn rows_of(answer: Option<&Answer>) -> Result<Option<&Rows>, DecodeError> {
    match answer {
        Some(Answer::Rows(rows)) => Ok(Some(rows)),
        Some(Answer::Error { code: Some(code), .. }) if REFUSED.contains(code) => Ok(None),
        Some(Answer::Error { message, .. }) => Err(DecodeError::Failed(message.clone())),
        None => Err(malformed("replication", "a read missing")),
    }
}

/// A time given as microseconds since the Unix epoch, 0 (the server's zero date) for never.
fn moment(row: &Row, name: &str) -> Result<Option<Timestamp>, DecodeError> {
    Ok(row.time(name)?.filter(|t| t.0 > 0))
}

fn running(value: Option<&str>) -> Running {
    match value {
        Some("Yes") => Running::Yes,
        Some("No") => Running::No,
        Some("Connecting") => Running::Connecting,
        Some(other) => Running::Other(other.to_owned()),
        None => Running::Unknown,
    }
}

/// A channel's row; `gtids_only`: every transaction carries a GTID (`gtid_mode` `ON`), so the
/// GTID sets account for all the channel received (else they are not read as such).
fn channel(cells: &[Option<String>], gtids_only: bool) -> Result<Upstream, DecodeError> {
    let r = Row::new(cells, "channels", REPLICA_STATUS)?;
    let text = |name: &str| r.string(name).filter(|t| !t.is_empty());
    let mut errors = Vec::new();
    for (side, code, message, at) in [
        (Side::Receiver, "Last_IO_Errno", "Last_IO_Error", "Last_IO_Error_Timestamp"),
        (Side::Applier, "Last_SQL_Errno", "Last_SQL_Error", "Last_SQL_Error_Timestamp"),
    ] {
        let code = r.uint(code)?.unwrap_or(0);
        if code != 0 {
            let code = u32::try_from(code).map_err(|_| malformed("channels", format!("error code {code}")))?;
            errors.push(ChannelError { side, code, message: text(message).unwrap_or_default(), at: text(at) });
        }
    }
    let source = match (text("Source_Host"), r.uint("Source_Port")?) {
        (Some(host), Some(port)) if port > 0 => Some(format!("{host}:{port}")),
        (host, _) => host,
    };
    let mut channel = Upstream {
        channel: text("Channel_Name"),
        source,
        slot: None,
        receiver: running(r.text("Replica_IO_Running")),
        receiver_state: text("Replica_IO_State"),
        applier: running(r.text("Replica_SQL_Running")),
        applier_state: text("Replica_SQL_Running_State"),
        received: None,
        applied: None,
        // The server writes the sets over several lines: kept as they are, read as sets.
        received_set: r.string("Retrieved_Gtid_Set"),
        executed_set: r.string("Executed_Gtid_Set"),
        gtids_only,
        gtids_left: None,
        source_read: log_pos(&r, "Source_Log_File", "Read_Source_Log_Pos")?,
        source_applied: log_pos(&r, "Relay_Source_Log_File", "Exec_Source_Log_Pos")?,
        applier_behind: r.uint("Seconds_Behind_Source")?,
        delay: r.uint("SQL_Delay")?,
        last_message: None,
        last_received: None,
        last_applied: None,
        errors,
        visible: Visibility::Full,
    };
    channel.count_gtids();
    Ok(channel)
}

/// A place in the source's binary log, `None` when the server says none: no file yet, or
/// `INVALID` (a `GTID_ONLY` channel keeps no positions: after a restart both read `INVALID`, 0,
/// which says nothing of what is left).
fn log_pos(r: &Row, file: &str, pos: &str) -> Result<Option<LogPos>, DecodeError> {
    Ok(match (r.string(file).filter(|f| !f.is_empty() && f != "INVALID"), r.uint(pos)?) {
        (Some(file), Some(pos)) => Some(LogPos { file, pos }),
        _ => None,
    })
}

fn replica(cells: &[Option<String>]) -> Result<Replica, DecodeError> {
    let r = Row::new(cells, "replicas", REPLICAS)?;
    Ok(Replica {
        id: r.int("ID")?,
        name: None,
        client: r.string("HOST").filter(|h| !h.is_empty()),
        state: r.string("STATE").filter(|s| !s.is_empty()),
        sync: None,
        sent: None,
        written: None,
        flushed: None,
        replayed: None,
        write_lag: None,
        flush_lag: None,
        replay_lag: None,
        reply: None,
        user: r.string("USER"),
        visible: Visibility::Full,
    })
}
