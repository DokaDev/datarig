//! The monitor's replication statements (MySQL 8.0.17 on), each giving the columns its
//! decoder in the core names (`monitor::decode::mysql::replication`). The driver sends them one
//! by one; each may be refused for lack of a privilege, which is an answer, not a failure.
//!
//! The channels' statement is chosen by version, never by trying both: from 8.0.22 `SHOW REPLICA
//! STATUS`, before `SHOW SLAVE STATUS` (the old name is gone from 8.4). It gives more columns
//! than the decoder reads and names them by version, so the driver keeps the decoder's columns
//! by name ([`Statement::columns`]). The replicas come from the session list, not `SHOW
//! REPLICAS`, which gives their passwords where `show_replica_auth_info` is on. None touches a
//! user's table; none reads a connection's password or options.

use super::NOW;
use datarig_core::monitor::MySqlMonitor;
use datarig_core::monitor::decode::mysql::replication::{REPLICA_STATUS, Read, SLAVE_STATUS, THREADS_LOST};

/// A statement of a replication read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Statement {
    pub read: Read,
    pub sql: String,
    /// The columns to keep, in the decoder's order, by the name the statement gives them; `None`:
    /// the statement gives exactly the decoder's columns.
    pub columns: Option<&'static [&'static str]>,
}

/// The new names for replication (`SHOW REPLICA STATUS`): 8.0.22 on.
fn new_names(my: &MySqlMonitor) -> bool {
    my.version >= 80022
}

/// A time column as microseconds since the Unix epoch (the zero date reads 0).
fn us(column: &str) -> String {
    format!("CAST(UNIX_TIMESTAMP({column}) * 1000000 AS SIGNED)")
}

/// The statements of a replication read, in this order: the clock and `@@gtid_mode`; the
/// server's channels (needs `REPLICATION CLIENT`); when each channel last heard from its
/// source, fetched a transaction and applied one (needs `SELECT` on those tables, which are
/// filled with the performance schema off too); where the replicas come from `performance_schema.processlist`, whether it
/// lost threads; the replicas connected to it (their dump threads in the session list: every
/// account's need `PROCESS`); the members of its replication group.
pub(crate) fn read(my: &MySqlMonitor) -> Vec<Statement> {
    let new = new_names(my);
    let mut statements = vec![
        Statement { read: Read::Clock, sql: format!("SELECT {NOW} AS now, @@gtid_mode AS gtid_mode"), columns: None },
        Statement {
            read: Read::Channels,
            sql: if new { "SHOW REPLICA STATUS" } else { "SHOW SLAVE STATUS" }.into(),
            columns: Some(if new { REPLICA_STATUS } else { SLAVE_STATUS }),
        },
    ];
    // The replication tables are filled with the performance schema off too.
    statements.push(Statement {
        read: Read::ChannelTimes,
        sql: format!(
            "SELECT c.CHANNEL_NAME, {heartbeat} AS last_heartbeat, {queued} AS last_queued, \
             (SELECT {applied} FROM performance_schema.replication_applier_status_by_worker w \
             WHERE w.CHANNEL_NAME = c.CHANNEL_NAME) AS last_applied \
             FROM performance_schema.replication_connection_status c ORDER BY c.CHANNEL_NAME",
            heartbeat = us("c.LAST_HEARTBEAT_TIMESTAMP"),
            queued = us("c.LAST_QUEUED_TRANSACTION_END_QUEUE_TIMESTAMP"),
            applied = us("MAX(w.LAST_APPLIED_TRANSACTION_END_APPLY_TIMESTAMP)"),
        ),
        columns: None,
    });
    // The replicas come from `performance_schema.processlist` where the session list does: then
    // whether it could track every thread tells whether the list is whole (`SHOW GLOBAL STATUS`
    // needs no privilege; a refusal of it leaves the rest).
    if my.ps_processlist() {
        statements.push(Statement {
            read: Read::ThreadsLost,
            sql: format!("SHOW GLOBAL STATUS WHERE Variable_name = '{THREADS_LOST}'"),
            columns: None,
        });
    }
    // The replicas connected now, from the session list as the tick reads it: `SHOW REPLICAS`
    // would give their passwords too on a server with `show_replica_auth_info` on.
    let source = if my.ps_processlist() { "performance_schema.processlist" } else { "information_schema.PROCESSLIST" };
    statements.push(Statement {
        read: Read::Replicas,
        sql: format!(
            "SELECT p.ID, p.USER, p.HOST, p.STATE FROM {source} p \
             WHERE p.COMMAND IN ('Binlog Dump', 'Binlog Dump GTID') ORDER BY p.ID"
        ),
        columns: None,
    });
    statements.push(Statement {
        read: Read::GroupMembers,
        sql: "SELECT CONCAT(m.MEMBER_HOST, ':', m.MEMBER_PORT) AS address, m.MEMBER_STATE, m.MEMBER_ROLE, \
             m.MEMBER_ID = @@server_uuid AS this FROM performance_schema.replication_group_members m \
             WHERE m.MEMBER_ID <> '' ORDER BY m.MEMBER_HOST, m.MEMBER_PORT"
            .into(),
        columns: None,
    });
    statements
}
