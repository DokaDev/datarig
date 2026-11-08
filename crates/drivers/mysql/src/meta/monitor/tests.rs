use super::*;
use datarig_core::monitor::Flavour;
use datarig_core::monitor::decode::mysql::{
    CLOCK, METADATA_LOCKS, NAME_VALUE, PROBE, PROBE_LOCKS, PROBE_STATEMENTS, QUERY_TEXT, ROW_LOCK_WAITS, SERVER_INFO,
    SESSIONS,
};
use std::time::Duration;

fn my(version: u32, performance_schema: bool, readable: Readable) -> MySqlMonitor {
    MySqlMonitor {
        version,
        performance_schema,
        user: "datarig".into(),
        process: Readable::Unknown,
        innodb_trx: readable,
        metadata_locks: readable,
        statements: readable,
        flavour: Flavour::Standard,
    }
}

const IDLE: SessionsWanted = SessionsWanted { idle: true };
const NO_IDLE: SessionsWanted = SessionsWanted { idle: false };

/// Every statement of a version, each with the shape its decoder reads.
fn every(m: &MySqlMonitor) -> Vec<(&'static str, String, &'static [&'static str])> {
    let probe = probe();
    let shapes: [&'static [&'static str]; 6] =
        [PROBE, NAME_VALUE, &["PRIVILEGE_TYPE"], &["trx_id"], PROBE_LOCKS, PROBE_STATEMENTS];
    let mut out: Vec<_> = probe.into_iter().zip(shapes).map(|(sql, shape)| ("probe", sql, shape)).collect();
    let info = server_info();
    out.push(("server info", info[0].clone(), SERVER_INFO));
    out.push(("server info uptime", info[1].clone(), NAME_VALUE));
    let c = counters(m);
    out.push(("clock", c[0].clone(), CLOCK));
    out.push(("status", c[1].clone(), NAME_VALUE));
    out.push(("sessions", sessions(m, IDLE).remove(0), SESSIONS));
    out.push(("sessions without idle", sessions(m, NO_IDLE).remove(0), SESSIONS));
    out.push(("query text", query_text(4211), QUERY_TEXT));
    out.push(("session before an action", guard(m, 63114), SESSIONS));
    out.push(("session after an action", session(m, 63114), SESSIONS));
    out.push(("row lock waits", row_lock_waits(m), ROW_LOCK_WAITS));
    out.push(("metadata lock waits", metadata_lock_waits(), METADATA_LOCKS));
    out
}

fn variants() -> [(&'static str, MySqlMonitor); 4] {
    [
        ("8_0_17", my(80017, true, Readable::Yes)),
        ("8_0_22_no_trx", my(80022, true, Readable::Refused)),
        ("8_4", my(80411, true, Readable::Yes)),
        ("8_4_no_performance_schema", my(80411, false, Readable::Yes)),
    ]
}

#[test]
fn statements_per_version() {
    for (name, m) in variants() {
        let text: Vec<String> = every(&m).iter().map(|(n, sql, _)| format!("-- {n}\n{sql}\n")).collect();
        insta::assert_snapshot!(format!("mysql_{name}"), text.join("\n"));
    }
}

#[test]
fn every_provider_marker_is_asked_for_and_never_the_host_name() {
    let probe = probe().join("\n");
    for (marker, _) in MYSQL_MARKERS {
        match marker {
            Marker::Variable(name) => assert!(probe.contains(&format!("'{name}'")), "{name}"),
            Marker::Basedir(_) => assert!(probe.contains("'basedir'")),
            other => panic!("{other:?} is not a MySQL marker"),
        }
    }
    assert!(!probe.contains("hostname"));
    assert!(super::probe()[2].contains("PRIVILEGE_TYPE = 'PROCESS'"), "SUPER does not show sessions");
}

#[test]
fn idle_sessions_are_left_out_only_when_their_transactions_are_known() {
    let with = sessions(&my(80411, true, Readable::Yes), NO_IDLE).join("");
    assert!(with.contains("p.COMMAND <> 'Sleep' OR t.trx_id IS NOT NULL OR l.id IS NOT NULL"));
    for unknown in [Readable::Refused, Readable::Unknown] {
        let without = sessions(&my(80411, true, unknown), NO_IDLE).join("");
        assert!(!without.contains("WHERE"), "{without}");
    }
    // InnoDB transactions known, metadata locks not: a session may still hold a transaction.
    let locks_unknown = MySqlMonitor { metadata_locks: Readable::Refused, ..my(80411, true, Readable::Yes) };
    assert!(!sessions(&locks_unknown, NO_IDLE)[0].contains("p.COMMAND <> 'Sleep'"));
    assert!(sessions(&my(80021, true, Readable::Yes), IDLE)[0].contains("information_schema.PROCESSLIST"));
    assert!(sessions(&my(80022, true, Readable::Yes), IDLE)[0].contains("performance_schema.processlist"));
}

#[test]
fn a_running_statement_is_named_by_its_event_only_when_the_events_are_read() {
    let with = sessions(&my(80411, true, Readable::Yes), IDLE).remove(0);
    assert!(with.contains("IF(pt.INSTRUMENTED = 'YES', e.EVENT_ID, NULL) AS EVENT_ID"));
    assert!(with.contains("e.NESTING_EVENT_LEVEL = 0 AND e.END_EVENT_ID IS NULL"));
    let unknown = MySqlMonitor { statements: Readable::Unknown, ..my(80411, true, Readable::Yes) };
    let off = my(80411, false, Readable::Yes);
    for m in [unknown, off] {
        let without = sessions(&m, IDLE).remove(0);
        assert!(without.contains("NULL AS THREAD_ID, NULL AS EVENT_ID") && !without.contains("events_statements"));
    }
    assert!(with.contains("MD5(p.INFO) AS info_md5"), "the whole text, not the list's start of it");
    assert!(probe()[5].contains("'events_statements_current'"));
}

#[test]
fn a_tick_reads_what_is_wanted_in_order() {
    let m = my(80411, true, Readable::Yes);
    let limit = Duration::from_secs(2);
    assert_eq!(tick(&m, &Want { sessions: None, locks: false, server: false, limit }), counters(&m));
    let statements = tick(&m, &Want { sessions: Some(IDLE), locks: false, server: true, limit });
    assert_eq!(statements.len(), 5);
    assert_eq!(statements[..2], counters(&m)[..]);
    assert_eq!(statements[2..3], sessions(&m, IDLE)[..]);
    assert_eq!(statements[3..], server_info()[..]);
    assert_eq!(super::limit(limit), "SET SESSION max_execution_time = 2000");
    assert_eq!(super::limit(Duration::ZERO), "SET SESSION max_execution_time = 1");
}

/// Each statement of every variant gives exactly the columns its decoder reads, in order, on
/// the real server (`DATARIG_TEST_MYSQL_ADMIN_URL`, an account that may read them all). They
/// only read; the older versions' statements read tables the newer servers still have.
#[tokio::test]
async fn every_statement_gives_its_decoders_columns() {
    let url = match std::env::var("DATARIG_TEST_MYSQL_ADMIN_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            let required = std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1");
            assert!(!required, "DATARIG_TEST_MYSQL_ADMIN_URL must be set");
            eprintln!("SKIPPED every_statement_gives_its_decoders_columns: set DATARIG_TEST_MYSQL_ADMIN_URL");
            return;
        }
    };
    use mysql_async::prelude::Queryable;
    let opts =
        mysql_async::OptsBuilder::from_opts(mysql_async::Opts::from_url(&url).expect("a URL")).prefer_socket(false);
    let mut conn = mysql_async::Conn::new(opts).await.expect("connect");
    for (variant, m) in variants() {
        for (name, sql, shape) in every(&m) {
            let result = conn.query_iter(sql.as_str()).await.unwrap_or_else(|e| panic!("{variant} {name}: {e}"));
            let columns: Vec<String> = result.columns_ref().iter().map(|c| c.name_str().into_owned()).collect();
            drop(result.drop_result().await);
            assert_eq!(columns, shape, "{variant} {name}");
        }
    }
    let _ = conn.disconnect().await;
}

/// The statements that act, and the read of a statement's end.
#[test]
fn action_statements() {
    let text = [
        ("cancel", kill(ActionKind::Cancel, 63114)),
        ("terminate", kill(ActionKind::Terminate, 63114)),
        ("procedure", call("mysql.rds_kill_query", 63114)),
        ("statement errno", statement_errno(63154, 1207)),
    ]
    .map(|(name, sql)| format!("-- {name}\n{sql}\n"));
    insta::assert_snapshot!("actions", text.join("\n"));
}

#[test]
fn the_read_before_an_action_tells_the_statement_as_the_tick_does() {
    let m = my(80411, true, Readable::Yes);
    let before = guard(&m, 63114);
    let tick = sessions(&m, IDLE).remove(0);
    for part in [
        "IF(pt.INSTRUMENTED = 'YES', e.EVENT_ID, NULL) AS EVENT_ID",
        "e.NESTING_EVENT_LEVEL = 0 AND e.END_EVENT_ID IS NULL",
        "MD5(p.INFO) AS info_md5",
        "p.COMMAND, p.TIME",
        "FROM performance_schema.processlist p",
    ] {
        assert!(before.contains(part) && tick.contains(part), "{part}");
    }
    assert!(before.contains("WHERE p.ID IN (63114, CONNECTION_ID())"), "{before}");
    assert!(!before.contains("INNODB_TRX") && !before.contains("metadata_locks"), "{before}");
    let old = guard(&my(80017, true, Readable::Yes), 1);
    assert!(old.contains("FROM information_schema.PROCESSLIST p"), "{old}");
    let weak = guard(&my(80411, false, Readable::Yes), 1);
    assert!(weak.contains("NULL AS THREAD_ID, NULL AS EVENT_ID") && weak.contains("MD5(p.INFO)"), "{weak}");
    assert!(session(&m, 63114).contains("INNODB_TRX"), "the follow-up reads the tick's columns");
}

#[test]
fn the_lock_reads_follow_what_the_account_may_read() {
    let m = my(80411, true, Readable::Yes);
    assert_eq!(locks(&m), [row_lock_waits(&m), metadata_lock_waits()]);
    for unreadable in [Readable::Refused, Readable::Unknown] {
        assert_eq!(locks(&MySqlMonitor { metadata_locks: unreadable, ..m.clone() }), [row_lock_waits(&m)]);
    }
    // Never every lock the engine holds: `data_locks` is not read.
    let rows = row_lock_waits(&m);
    // The blocking transaction's session now (a detached XA transaction has none), not the
    // thread that took the lock.
    assert!(rows.contains("NULLIF(bx.trx_mysql_thread_id, 0) AS blocker"), "{rows}");
    assert!(!rows.contains("BLOCKING_THREAD_ID"), "{rows}");
    let blind = row_lock_waits(&MySqlMonitor { innodb_trx: Readable::Refused, ..m.clone() });
    assert!(blind.contains("CAST(NULL AS UNSIGNED) AS blocker") && !blind.contains("INNODB_TRX"), "{blind}");
    assert!(rows.contains("FROM performance_schema.data_lock_waits w"), "{rows}");
    assert!(!rows.contains("data_locks "), "{rows}");
    // Other threads' locks on the object only; pending first, bounded.
    let mdl = metadata_lock_waits();
    assert!(mdl.contains("p.thread <> g.thread") && mdl.contains("ORDER BY LOCK_STATUS DESC"), "{mdl}");
    assert!(rows.ends_with(&format!("LIMIT {LOCK_ROWS}")) && mdl.ends_with(&format!("LIMIT {LOCK_ROWS}")));
}

#[test]
fn the_row_lock_waits_are_not_read_where_their_read_stalls_the_server() {
    let old = my(80037, true, Readable::Yes);
    assert_eq!(locks(&old), [metadata_lock_waits()], "8.0.37: the metadata lock waits only");
    let fixed = my(80038, true, Readable::Yes);
    assert_eq!(locks(&fixed), [row_lock_waits(&fixed), metadata_lock_waits()]);
}

#[test]
fn replication_statements_per_version() {
    for (name, version, ps) in [
        ("mysql_8_0_17", 80017, true),
        ("mysql_8_0_22", 80022, true),
        ("mysql_8_4", 80411, true),
        ("mysql_8_4_no_performance_schema", 80411, false),
    ] {
        let text: Vec<String> = replication::read(&my(version, ps, Readable::Yes))
            .iter()
            .map(|s| {
                format!(
                    "-- {:?}{}\n{}\n",
                    s.read,
                    s.columns.map_or(String::new(), |c| format!(" ({})", c.join(", "))),
                    s.sql
                )
            })
            .collect();
        insta::assert_snapshot!(format!("replication_{name}"), text.join("\n"));
    }
}

#[test]
fn replication_statements_are_chosen_by_version_and_the_performance_schema() {
    use datarig_core::monitor::decode::mysql::replication::{REPLICA_STATUS, Read, SLAVE_STATUS};
    let reads = |m: &MySqlMonitor| replication::read(m).into_iter().map(|s| s.read).collect::<Vec<_>>();
    let all = [Read::Clock, Read::Channels, Read::ChannelTimes, Read::ThreadsLost, Read::Replicas, Read::GroupMembers];
    assert_eq!(reads(&my(80411, true, Readable::Yes)), all);
    // The replication tables are filled with the performance schema off too: only the thread
    // loss (of the performance schema's session list) is not read then.
    assert_eq!(
        reads(&my(80411, false, Readable::Yes)),
        [Read::Clock, Read::Channels, Read::ChannelTimes, Read::Replicas, Read::GroupMembers]
    );
    // Before 8.0.22 the list comes from information_schema: no thread to lose there.
    assert!(!reads(&my(80021, true, Readable::Yes)).contains(&Read::ThreadsLost));
    let find = |m: &MySqlMonitor, r: Read| replication::read(m).into_iter().find(|s| s.read == r).expect("a read");
    let old = my(80021, true, Readable::Yes);
    let channels = find(&old, Read::Channels);
    assert_eq!((channels.sql.as_str(), channels.columns), ("SHOW SLAVE STATUS", Some(SLAVE_STATUS)));
    let replicas = find(&old, Read::Replicas);
    assert!(
        replicas.sql.contains("FROM information_schema.PROCESSLIST p") && replicas.columns.is_none(),
        "{replicas:?}"
    );
    let new = my(80022, true, Readable::Yes);
    let channels = find(&new, Read::Channels);
    assert_eq!((channels.sql.as_str(), channels.columns), ("SHOW REPLICA STATUS", Some(REPLICA_STATUS)));
    assert!(find(&new, Read::Replicas).sql.contains("FROM performance_schema.processlist p"));
    // Never `SHOW REPLICAS`, which gives the replicas' passwords where `show_replica_auth_info`
    // is on.
    for m in [my(80021, true, Readable::Yes), my(80411, true, Readable::Yes)] {
        assert!(replication::read(&m).iter().all(|s| !s.sql.contains("REPLICAS") && !s.sql.contains("SLAVE HOSTS")));
    }
}

/// Each replication statement for the server's own version gives the columns its decoder
/// reads, on the real server (`DATARIG_TEST_MYSQL_ADMIN_URL`, an account that may read them):
/// exactly, or (the `SHOW` statements, which give more) each of them by name.
#[tokio::test]
async fn every_replication_statement_gives_its_decoders_columns() {
    use datarig_core::monitor::decode::mysql::replication::{CHANNEL_TIMES, GROUP_MEMBERS, REPLICAS, Read};
    let url = match std::env::var("DATARIG_TEST_MYSQL_ADMIN_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            let required = std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1");
            assert!(!required, "DATARIG_TEST_MYSQL_ADMIN_URL must be set");
            eprintln!(
                "SKIPPED every_replication_statement_gives_its_decoders_columns: set DATARIG_TEST_MYSQL_ADMIN_URL"
            );
            return;
        }
    };
    use mysql_async::prelude::Queryable;
    let opts =
        mysql_async::OptsBuilder::from_opts(mysql_async::Opts::from_url(&url).expect("a URL")).prefer_socket(false);
    let mut conn = mysql_async::Conn::new(opts).await.expect("connect");
    let line: String = conn.query_first("SELECT VERSION()").await.expect("version").expect("a row");
    let mut parts = line.split(['.', '-']).map(|p| p.parse::<u32>().unwrap_or(0));
    let version = parts.next().unwrap_or(0) * 10000 + parts.next().unwrap_or(0) * 100 + parts.next().unwrap_or(0);
    for statement in replication::read(&my(version, true, Readable::Yes)) {
        let result =
            conn.query_iter(statement.sql.as_str()).await.unwrap_or_else(|e| panic!("{:?}: {e}", statement.read));
        let columns: Vec<String> = result.columns_ref().iter().map(|c| c.name_str().into_owned()).collect();
        drop(result.drop_result().await);
        match (statement.read, statement.columns) {
            (_, Some(wanted)) => {
                for name in wanted {
                    assert!(columns.iter().any(|c| c == name), "{:?}: no {name} in {columns:?}", statement.read);
                }
            }
            (Read::Clock, None) => assert_eq!(columns, datarig_core::monitor::decode::mysql::replication::STATE),
            (Read::ChannelTimes, None) => assert_eq!(columns, CHANNEL_TIMES),
            (Read::GroupMembers, None) => assert_eq!(columns, GROUP_MEMBERS),
            (Read::Replicas, None) => assert_eq!(columns, REPLICAS),
            (Read::ThreadsLost, None) => assert_eq!(columns, NAME_VALUE),
            (read, None) => panic!("{read:?} has no shape"),
        }
    }
    let _ = conn.disconnect().await;
}
