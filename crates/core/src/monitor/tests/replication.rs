//! Replication: log positions, GTID sets, roles and lags, and the rows of real servers read
//! into the model (fixtures recorded from primary and replica pairs; the group replication
//! members are synthetic, and say so).

use super::super::decode::mysql::replication::Read;
use super::super::replication::{self, Running, gtids_missing};
use super::*;

macro_rules! fixture {
    ($name:literal) => {
        load(include_str!(concat!("../fixtures/", $name, ".json")))
    };
}

const A: &str = "ced11181-db1f-11f1-9e15-967323c11f94";
const B: &str = "ccf08818-db1f-11f1-bcb9-6aa192f2d4c5";

#[test]
fn log_positions_read_as_the_server_writes_them() {
    assert_eq!(Lsn(0x3C8_4EC8).to_string(), "0/3C84EC8");
    assert_eq!(Lsn((0x16 << 32) | 0xB374_D848).to_string(), "16/B374D848");
    assert_eq!(Lsn(10).bytes_to(Lsn(25)), Some(15));
    assert_eq!(Lsn(25).bytes_to(Lsn(10)), None);
}

#[test]
fn gtid_sets_count_what_was_received_and_not_executed() {
    assert_eq!(gtids_missing(&format!("{A}:1-10"), &format!("{A}:1-10")), Some(0));
    assert_eq!(gtids_missing(&format!("{A}:1-10"), &format!("{A}:1-5")), Some(5));
    // As the server writes them: several sources over several lines, in any case.
    let executed = format!("{B}:1-5,\n{}:1-7:9", A.to_uppercase());
    assert_eq!(gtids_missing(&format!("{A}:1-10"), &executed), Some(2));
    // Executed intervals that overlap or touch are one.
    assert_eq!(gtids_missing(&format!("{A}:1-10"), &format!("{A}:1-3:2-6,{A}:7-8")), Some(2));
    // Another source's transactions do not count for this one.
    assert_eq!(gtids_missing(&format!("{A}:1-3"), &format!("{B}:1-3")), Some(3));
    // Tagged transactions (8.3 on) are their own.
    assert_eq!(gtids_missing(&format!("{A}:1-3:batch:1-2"), &format!("{A}:1-3")), Some(2));
    // A tag holds for the intervals after it.
    assert_eq!(gtids_missing(&format!("{A}:1-3:batch:1-2"), &format!("{A}:batch:1-2,{A}:1-3")), Some(0));
    assert_eq!(gtids_missing(&format!("{A}:1-3"), &format!("{A}:batch:1-3")), Some(3));
    assert_eq!(gtids_missing("", ""), Some(0));
    // Many intervals: one pass over each list (a set of 200,000 intervals against another).
    let received: String = (0..200_000u64).map(|i| format!("{A}:{}", 2 * i + 1)).collect::<Vec<_>>().join(",");
    let executed: String = (0..200_000u64).map(|i| format!("{A}:{}", 2 * i + 2)).collect::<Vec<_>>().join(",");
    let started = std::time::Instant::now();
    assert_eq!(gtids_missing(&received, &executed), Some(200_000));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    // Not a set: not counted (never 0).
    assert_eq!(gtids_missing("not a set", ""), None);
    assert_eq!(gtids_missing(&format!("{A}:5-1"), ""), None);
}

fn upstream() -> replication::Upstream {
    replication::Upstream {
        channel: None,
        source: None,
        slot: None,
        receiver: Running::Yes,
        receiver_state: None,
        applier: Running::Yes,
        applier_state: None,
        received: None,
        applied: None,
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
        last_applied: None,
        errors: Vec::new(),
        visible: Visibility::Full,
    }
}

#[test]
fn a_replica_says_how_far_it_is_behind_only_as_far_as_it_can_tell() {
    let pg = |received, applied| replication::Upstream {
        received: Some(Lsn(received)),
        applied: Some(Lsn(applied)),
        ..upstream()
    };
    assert_eq!(pg(100, 100).lag(), Lag::CaughtUp);
    assert_eq!(pg(100, 40).lag(), Lag::Behind { bytes: Some(60), transactions: None, seconds: None });
    // Paused: behind by what it has not replayed, whether it runs or not.
    let paused = replication::Upstream { applier: Running::Paused, ..pg(100, 40) };
    assert_eq!(paused.lag(), Lag::Behind { bytes: Some(60), transactions: None, seconds: None });
    let paused = replication::Upstream { applier: Running::Paused, received: None, ..pg(100, 40) };
    assert_eq!(paused.lag(), Lag::Stopped);
    // Nothing received to compare with: not known (never caught up).
    assert_eq!(replication::Upstream { applied: Some(Lsn(5)), ..upstream() }.lag(), Lag::Unknown);

    let my = |received: &str, executed: &str, behind| {
        let mut u = replication::Upstream {
            received_set: Some(received.into()),
            executed_set: Some(executed.into()),
            gtids_only: true,
            applier_behind: behind,
            ..upstream()
        };
        u.count_gtids();
        u
    };
    let at = |file: &str, pos| Some(replication::LogPos { file: file.into(), pos });
    let pos = |u: replication::Upstream, read, applied| replication::Upstream {
        source_read: read,
        source_applied: applied,
        ..u
    };
    // The receiver's and the applier's places in the source's log tell: the same place, nothing
    // is left, whatever else says (seconds, sets, a stopped applier).
    let all = format!("{A}:1-10");
    assert_eq!(pos(my(&all, &all, Some(0)), at("bin.000003", 3256), at("bin.000003", 3256)).lag(), Lag::CaughtUp);
    let stopped = replication::Upstream { applier: Running::No, ..my(&all, &all, None) };
    assert_eq!(pos(stopped, at("bin.000003", 3256), at("bin.000003", 3256)).lag(), Lag::CaughtUp);
    // Apart: behind, by the bytes between them in one file, with the sets' count and the seconds.
    // `Seconds_Behind_Source` 0 with events of the same second not applied is still behind.
    assert_eq!(
        pos(my(&all, &format!("{A}:1-9"), Some(0)), at("bin.000003", 3256), at("bin.000003", 3000)).lag(),
        Lag::Behind { bytes: Some(256), transactions: Some(1), seconds: Some(0) }
    );
    assert_eq!(
        pos(my(&all, &all, Some(3)), at("bin.000004", 120), at("bin.000003", 3000)).lag(),
        Lag::Behind { bytes: None, transactions: None, seconds: Some(3) }
    );
    // A `GTID_ONLY` channel keeps no positions: `INVALID` reads as none (both are `INVALID`, 0
    // after a restart, which proves nothing).
    let mut answers = mysql_answers(fixture!("replication/my84_replica"), &ALL_MYSQL);
    if let Answer::Rows(rows) = &mut answers[1].1 {
        let n = rows[0].len();
        for i in [n - 4, n - 2] {
            rows[0][i] = Some("INVALID".into());
        }
        for i in [n - 3, n - 1] {
            rows[0][i] = Some("0".into());
        }
    }
    let r = decode::mysql::replication::read(&self::my(true), &answers).unwrap();
    let c = &r.upstream.as_ref().unwrap()[0];
    assert_eq!((c.source_read.clone(), c.source_applied.clone()), (None, None));
    // Without the positions: 0 seconds proves nothing (events of the same second may wait).
    assert_eq!(my(&all, &all, Some(0)).lag(), Lag::Unknown);
    // NULL while the applier runs: it applied all the receiver fetched, which then stopped.
    assert_eq!(my(&all, &all, None).lag(), Lag::CaughtUp);
    assert_eq!(
        my(&all, &format!("{A}:1-4"), Some(42)).lag(),
        Lag::Behind { bytes: None, transactions: Some(6), seconds: Some(42) }
    );
    let stopped = replication::Upstream { applier: Running::No, ..my(&all, &format!("{A}:1-4"), None) };
    assert_eq!(stopped.lag(), Lag::Behind { bytes: None, transactions: Some(6), seconds: None });
    // The sets never prove nothing is left (a channel may take anonymous transactions and give
    // them GTIDs as it applies them): a stopped applier is stopped.
    assert_eq!(replication::Upstream { applier: Running::No, ..my(&all, &all, None) }.lag(), Lag::Stopped);
    // Where a transaction may come without a GTID (`gtid_mode` not `ON`) the sets count nothing.
    let mut permissive = replication::Upstream { gtids_only: false, ..my(&all, &format!("{A}:1-4"), None) };
    permissive.count_gtids();
    assert_eq!(permissive.gtids_left, None);
    assert_eq!(permissive.lag(), Lag::CaughtUp, "NULL while running: the seconds tell");
    assert_eq!(replication::Upstream { applier: Running::No, ..permissive }.lag(), Lag::Stopped);
    // Without GTIDs both sets are empty: they say nothing.
    assert_eq!(my("", "", Some(90)).lag(), Lag::Behind { bytes: None, transactions: None, seconds: Some(90) });
    assert_eq!(replication::Upstream { applier: Running::No, ..my("", "", None) }.lag(), Lag::Stopped);
    // Sets that do not read: the seconds alone; an applier in no known state is not known.
    assert_eq!(my("?", "?", Some(9)).lag(), Lag::Behind { bytes: None, transactions: None, seconds: Some(9) });
    assert_eq!(replication::Upstream { applier: Running::Unknown, ..my("?", "?", None) }.lag(), Lag::Unknown);
    // What the account may not see is not known.
    let hidden = replication::Upstream { visible: Visibility::Hidden, applier: Running::Unknown, ..upstream() };
    assert_eq!(hidden.lag(), Lag::Unknown);
}

#[test]
fn the_role_is_not_known_when_what_tells_it_was_refused() {
    let r = |in_recovery, upstream: Option<usize>, replicas: Option<usize>| Replication {
        time: Timestamp(0),
        in_recovery,
        position: None,
        upstream: upstream.map(|n| vec![upstream_one(); n]),
        replicas: replicas.map(|n| vec![replica_one(); n]),
        slots: None,
        subscriptions: None,
        members: Vec::new(),
        hidden: Vec::new(),
    };
    assert_eq!(r(Some(true), Some(1), Some(0)).role(), Role::Replica);
    assert_eq!(r(Some(false), Some(0), Some(2)).role(), Role::Primary);
    assert_eq!(r(Some(false), Some(0), Some(0)).role(), Role::Standalone);
    // MySQL: told by the channels and the registered replicas.
    assert_eq!(r(None, Some(1), Some(1)).role(), Role::Replica);
    assert_eq!(r(None, Some(0), Some(1)).role(), Role::Primary);
    // Replicas, and its own source not readable: it may take changes too.
    assert_eq!(r(None, None, Some(1)).role(), Role::Unknown);
    assert_eq!(r(None, None, Some(0)).role(), Role::Unknown);
    assert_eq!(r(None, Some(0), None).role(), Role::Unknown);
    assert_eq!(r(None, Some(1), None).role(), Role::Replica);
    // A group's members tell, with no channel and no replica connected.
    let member = |role: &str, this| replication::Member {
        address: "gr1:3306".into(),
        state: "ONLINE".into(),
        role: Some(role.into()),
        this,
    };
    let group = |members| Replication { members, ..r(None, Some(0), Some(0)) };
    assert_eq!(group(vec![member("PRIMARY", true), member("SECONDARY", false)]).role(), Role::Primary);
    assert_eq!(group(vec![member("PRIMARY", false), member("SECONDARY", true)]).role(), Role::Replica);
    assert_eq!(group(Vec::new()).role(), Role::Standalone);
    // The group not readable: not "not replicating".
    let hidden = Replication {
        hidden: vec![Hidden { part: Part::GroupMembers, need: Need::Grant("SELECT on performance_schema") }],
        ..group(Vec::new())
    };
    assert_eq!(hidden.role(), Role::Unknown);
    // Replicas connected, the group not readable: it may be a group's secondary.
    let hidden = Replication {
        hidden: vec![Hidden { part: Part::GroupMembers, need: Need::Grant("SELECT on performance_schema") }],
        ..r(None, Some(0), Some(1))
    };
    assert_eq!(hidden.role(), Role::Unknown);
}

fn upstream_one() -> replication::Upstream {
    upstream()
}

fn replica_one() -> replication::Replica {
    replication::Replica {
        id: None,
        name: None,
        client: None,
        state: None,
        sync: None,
        sent: None,
        written: None,
        flushed: None,
        replayed: None,
        write_lag: None,
        flush_lag: None,
        replay_lag: None,
        reply: None,
        user: None,
        visible: Visibility::Full,
    }
}

/// What the probe of the account found: `PROCESS` held (root) or not seen.
fn my(process: bool) -> MySqlMonitor {
    MySqlMonitor {
        version: 80411,
        performance_schema: true,
        user: "root".into(),
        process: if process { Readable::Yes } else { Readable::Unknown },
        innodb_trx: Readable::Yes,
        metadata_locks: Readable::Yes,
        statements: Readable::Yes,
        flavour: Flavour::Standard,
    }
}

fn mysql_answers(json: Vec<Answer>, reads: &[Read]) -> Vec<(Read, Answer)> {
    assert_eq!(json.len(), reads.len());
    reads.iter().copied().zip(json).collect()
}

/// The reads of 8.0.22 on with the performance schema on, and of 8.0.21 (its session list is
/// not the performance schema's: no thread to lose).
const ALL_MYSQL: [Read; 6] =
    [Read::Clock, Read::Channels, Read::ChannelTimes, Read::ThreadsLost, Read::Replicas, Read::GroupMembers];
const MYSQL_80: [Read; 5] = [Read::Clock, Read::Channels, Read::ChannelTimes, Read::Replicas, Read::GroupMembers];

#[test]
fn postgres_primary_with_a_streaming_standby_and_slots() {
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg17_primary"))).unwrap();
    assert_eq!(r.role(), Role::Primary);
    assert_eq!(r.in_recovery, Some(false));
    assert!(r.hidden.is_empty());
    let replicas = r.replicas.as_ref().unwrap();
    // The standby, and a logical subscriber's WAL sender (another server's subscription).
    assert_eq!(replicas.iter().map(|r| r.name.as_deref().unwrap_or("")).collect::<Vec<_>>(), ["walreceiver", "zz_sub"]);
    let s = &replicas[0];
    assert_eq!(
        (s.name.as_deref(), s.state.as_deref(), s.sync.as_deref()),
        (Some("walreceiver"), Some("streaming"), Some("async"))
    );
    assert!(s.client.as_deref().is_some_and(|c| c.ends_with(char::is_numeric)), "{:?}", s.client);
    assert_eq!(s.visible, Visibility::Full);
    assert!(s.replayed.is_some() && s.reply.is_some());
    assert!(matches!(s.lag(r.position), Lag::CaughtUp | Lag::Behind { .. }));
    let slots = r.slots.as_ref().unwrap();
    let names: Vec<&str> = slots.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["standby1", "zz_idle_slot", "zz_logical", "zz_sub"]);
    let idle = &slots[1];
    assert!(!idle.active && idle.kept.unwrap() > 0, "an away consumer's slot keeps log: {idle:?}");
    assert_eq!(idle.wal_status.as_deref(), Some("reserved"));
    let logical = &slots[2];
    assert_eq!(
        (logical.kind.as_str(), logical.plugin.as_deref(), logical.database.as_deref()),
        ("logical", Some("test_decoding"), Some("postgres"))
    );
    assert_eq!(r.upstream.as_deref(), Some(&[][..]));
    assert_eq!(r.subscriptions.as_deref(), Some(&[][..]));
}

#[test]
fn postgres_standby_streaming_and_paused() {
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg17_standby"))).unwrap();
    assert_eq!(r.role(), Role::Replica);
    let u = &r.upstream.as_ref().unwrap()[0];
    assert_eq!(u.receiver, Running::Yes);
    assert_eq!(u.slot.as_deref(), Some("standby1"));
    assert_eq!(u.source.as_deref(), Some("pg17p:5432"));
    assert!(u.last_message.is_some() && u.last_applied.is_some());
    assert_eq!(u.lag(), Lag::CaughtUp);
    assert_eq!(r.replicas.as_deref(), Some(&[][..]));

    // Replay paused while the primary wrote: received, not replayed.
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg17_standby_paused"))).unwrap();
    let u = &r.upstream.as_ref().unwrap()[0];
    assert_eq!(u.applier, Running::Paused);
    assert!(u.received > u.applied, "{u:?}");
    let behind = u.received.unwrap().0 - u.applied.unwrap().0;
    assert_eq!(u.lag(), Lag::Behind { bytes: Some(behind), transactions: None, seconds: None });

    // The primary gone: no WAL receiver, what was received stays known.
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg17_standby_no_receiver"))).unwrap();
    let u = &r.upstream.as_ref().unwrap()[0];
    assert_eq!((u.receiver.clone(), u.source.as_deref()), (Running::No, None));
    assert!(u.received.is_some());
}

#[test]
fn postgres_replication_hidden_from_an_account_without_read_all_stats() {
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg17_primary_unprivileged"))).unwrap();
    assert_eq!(r.hidden, [Hidden { part: Part::ReplicationDetails, need: Need::Grant("pg_read_all_stats") }]);
    let s = &r.replicas.as_ref().unwrap()[0];
    assert_eq!(s.visible, Visibility::Hidden);
    assert!(s.id.is_some() && s.state.is_none() && s.replayed.is_none());
    // Hidden is not "caught up".
    assert_eq!(s.lag(r.position), Lag::Unknown);
    // The slots every account may read.
    assert_eq!(r.slots.as_ref().unwrap().len(), 4);

    let r = decode::pg::replication::read(&rows(fixture!("replication/pg17_standby_unprivileged"))).unwrap();
    let u = &r.upstream.as_ref().unwrap()[0];
    assert_eq!((u.visible, u.receiver.clone()), (Visibility::Hidden, Running::Unknown));
    // The positions come from functions every account may call.
    assert!(u.received.is_some() && u.applied.is_some());
    assert_eq!(r.hidden.len(), 1);
}

#[test]
fn postgres_12_reads_the_same_without_slot_status() {
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg12_primary"))).unwrap();
    assert_eq!(r.role(), Role::Primary);
    assert!(r.slots.as_ref().unwrap().iter().all(|s| s.wal_status.is_none() && s.safe_wal_size.is_none()));
    let r = decode::pg::replication::read(&rows(fixture!("replication/pg12_standby"))).unwrap();
    assert_eq!(r.role(), Role::Replica);
    assert_eq!(r.upstream.as_ref().unwrap()[0].receiver, Running::Yes);
}

#[test]
fn a_postgres_logical_subscriber_takes_changes() {
    for fixture in [fixture!("replication/pg12_subscriber"), fixture!("replication/pg12_subscriber_unprivileged")] {
        let r = decode::pg::replication::read(&rows(fixture)).unwrap();
        assert_eq!(r.in_recovery, Some(false));
        assert_eq!(r.role(), Role::Replica, "it takes changes by its subscription, never \"not replicating\"");
        let subs = r.subscriptions.as_ref().unwrap();
        assert_eq!(subs.len(), 1);
        assert!(subs[0].running && subs[0].received.is_some() && subs[0].last_message.is_some(), "{subs:?}");
        assert_eq!(subs[0].name, "zz_sub");
    }
}

#[test]
fn postgres_replication_rows_of_another_shape_are_refused() {
    let mut all = rows(fixture!("replication/pg17_primary"));
    all.pop();
    assert!(matches!(decode::pg::replication::read(&all), Err(DecodeError::Malformed { .. })));
    let mut all = rows(fixture!("replication/pg17_primary"));
    all[1][0].pop();
    assert!(matches!(decode::pg::replication::read(&all), Err(DecodeError::Malformed { .. })));
}

#[test]
fn mysql_replica_and_source() {
    let r =
        decode::mysql::replication::read(&my(true), &mysql_answers(fixture!("replication/my84_replica"), &ALL_MYSQL))
            .unwrap();
    assert_eq!(r.role(), Role::Replica);
    assert!(r.hidden.is_empty(), "{:?}", r.hidden);
    let c = &r.upstream.as_ref().unwrap()[0];
    assert_eq!((c.channel.as_deref(), c.source.as_deref()), (None, Some("my84s:3306")));
    assert_eq!((c.receiver.clone(), c.applier.clone()), (Running::Yes, Running::Yes));
    assert_eq!(c.applier_behind, Some(0));
    assert!(c.received_set.is_some() && c.executed_set.is_some() && c.errors.is_empty());
    assert!(c.last_message.is_some(), "the heartbeat");
    assert_eq!(c.lag(), Lag::CaughtUp);
    assert_eq!(r.slots, None);

    let r =
        decode::mysql::replication::read(&my(true), &mysql_answers(fixture!("replication/my84_source"), &ALL_MYSQL))
            .unwrap();
    assert_eq!(r.role(), Role::Primary);
    let rep = &r.replicas.as_ref().unwrap()[0];
    assert_eq!(rep.user.as_deref(), Some("repl"));
    assert!(rep.client.as_deref().is_some_and(|c| c.contains(':')), "{rep:?}");
    assert!(rep.state.as_deref().is_some_and(|s| s.contains("sent all binlog")), "{rep:?}");
    assert_eq!(rep.lag(None), Lag::Unknown, "a source does not know its replicas' lag");
    // Without `PROCESS` seen, no dump thread listed says nothing.
    let r =
        decode::mysql::replication::read(&my(false), &mysql_answers(fixture!("replication/my84_replica"), &ALL_MYSQL))
            .unwrap();
    assert_eq!(r.replicas, None);
    assert_eq!(r.hidden, [Hidden { part: Part::Replicas, need: Need::Grant("PROCESS") }]);
    assert_eq!(r.role(), Role::Replica, "its channel tells");
    // The performance schema lost threads: a dump thread may be missing from the list.
    let mut answers = mysql_answers(fixture!("replication/my84_source"), &ALL_MYSQL);
    answers[3].1 = Answer::Rows(vec![vec![Some("Performance_schema_thread_instances_lost".into()), Some("3".into())]]);
    let r = decode::mysql::replication::read(&my(true), &answers).unwrap();
    assert_eq!(
        r.hidden,
        [Hidden { part: Part::Replicas, need: Need::Setting("performance_schema_max_thread_instances") }]
    );
    assert_eq!(r.replicas.as_ref().map(Vec::len), Some(1), "what was listed stays listed");
    assert_eq!(r.role(), Role::Primary);
    // The list itself refused (no `SELECT` on performance_schema.processlist): that grant.
    let mut refused = mysql_answers(fixture!("replication/my84_source"), &ALL_MYSQL);
    refused[4].1 = Answer::Error { code: Some(1142), message: "SELECT command denied".into() };
    let r = decode::mysql::replication::read(&my(true), &refused).unwrap();
    assert_eq!(r.hidden, [Hidden { part: Part::Replicas, need: Need::Grant("SELECT on performance_schema") }]);
    assert_eq!(r.replicas, None);
    // Refused (no privilege a `SHOW GLOBAL STATUS` needs, but a server may say so): whether the
    // list is whole is not known; the rest is read all the same.
    answers[3].1 = Answer::Error { code: Some(1227), message: "Access denied".into() };
    let r = decode::mysql::replication::read(&my(true), &answers).unwrap();
    assert_eq!(
        r.hidden,
        [Hidden { part: Part::Replicas, need: Need::Setting("performance_schema_max_thread_instances") }]
    );
    assert!(r.upstream.is_some());

    // Before 8.0.22: the old statements, the same model.
    let r =
        decode::mysql::replication::read(&my(true), &mysql_answers(fixture!("replication/my80_replica"), &MYSQL_80))
            .unwrap();
    assert_eq!(r.role(), Role::Replica);
    assert_eq!(r.upstream.as_ref().unwrap()[0].source.as_deref(), Some("my80s:3306"));
    let r = decode::mysql::replication::read(&my(true), &mysql_answers(fixture!("replication/my80_source"), &MYSQL_80))
        .unwrap();
    assert_eq!(r.replicas.as_ref().unwrap()[0].user.as_deref(), Some("repl"));
}

#[test]
fn mysql_replica_behind_stopped_and_failing() {
    // A delayed applier: transactions received and not applied yet, and its seconds.
    let r = decode::mysql::replication::read(
        &my(true),
        &mysql_answers(fixture!("replication/my84_replica_delayed"), &ALL_MYSQL),
    )
    .unwrap();
    let c = &r.upstream.as_ref().unwrap()[0];
    assert!(c.delay.is_some_and(|d| d > 0));
    match c.lag() {
        Lag::Behind { transactions: Some(n), seconds: Some(_), bytes: Some(b) } => assert!(n > 0 && b > 0),
        other => panic!("{other:?}"),
    }
    // The receiver stopped: the applier caught up with what it had, NULL seconds.
    let r = decode::mysql::replication::read(
        &my(true),
        &mysql_answers(fixture!("replication/my84_replica_io_stopped"), &ALL_MYSQL),
    )
    .unwrap();
    let c = &r.upstream.as_ref().unwrap()[0];
    assert_eq!((c.receiver.clone(), c.applier_behind), (Running::No, None));
    assert_eq!(c.lag(), Lag::CaughtUp);
    // The applier failed on a duplicate key: its error, and it applies nothing.
    let r = decode::mysql::replication::read(
        &my(true),
        &mysql_answers(fixture!("replication/my84_replica_error"), &ALL_MYSQL),
    )
    .unwrap();
    let c = &r.upstream.as_ref().unwrap()[0];
    assert_eq!(c.applier, Running::No);
    let e = c.errors.iter().find(|e| e.side == replication::Side::Applier).expect("the applier's error");
    assert_eq!(e.code, 1062);
    assert!(e.at.is_some());
    assert!(matches!(c.lag(), Lag::Behind { transactions: Some(1), seconds: None, .. }), "{:?}", c.lag());
}

#[test]
fn mysql_replication_refused_is_said_never_read_as_not_replicating() {
    let r = decode::mysql::replication::read(
        &my(false),
        &mysql_answers(fixture!("replication/my84_replica_unprivileged"), &ALL_MYSQL),
    )
    .unwrap();
    assert_eq!(r.upstream, None);
    assert_eq!(r.replicas, None);
    assert_eq!(r.role(), Role::Unknown);
    let parts: Vec<(Part, Need)> = r.hidden.iter().map(|h| (h.part, h.need.clone())).collect();
    assert_eq!(
        parts,
        [
            (Part::ReplicationSources, Need::Grant("REPLICATION CLIENT")),
            (Part::Replicas, Need::Grant("PROCESS")),
            (Part::GroupMembers, Need::Grant("SELECT on performance_schema")),
        ]
    );
    // Another failure is not a refusal: the read fails.
    let mut answers = mysql_answers(fixture!("replication/my84_replica"), &ALL_MYSQL);
    answers[1].1 = Answer::Error { code: Some(2013), message: "Lost connection".into() };
    assert!(matches!(decode::mysql::replication::read(&my(true), &answers), Err(DecodeError::Failed(_))));
    // Channel times refused with channels there: said; not read with the performance schema
    // off: said as the setting.
    let mut answers = mysql_answers(fixture!("replication/my84_replica"), &ALL_MYSQL);
    answers[2].1 = Answer::Error { code: Some(1142), message: "SELECT command denied".into() };
    let r = decode::mysql::replication::read(&my(true), &answers).unwrap();
    assert_eq!(r.hidden, [Hidden { part: Part::ReplicationTimes, need: Need::Grant("SELECT on performance_schema") }]);
    let answers: Vec<_> = mysql_answers(fixture!("replication/my84_replica"), &ALL_MYSQL)
        .into_iter()
        .filter(|(r, _)| !matches!(r, Read::ChannelTimes | Read::ThreadsLost | Read::GroupMembers))
        .collect();
    let r = decode::mysql::replication::read(&my(true), &answers).unwrap();
    assert_eq!(r.hidden, [Hidden { part: Part::ReplicationTimes, need: Need::Setting("performance_schema") }]);
    assert!(r.members.is_empty());
}

/// Synthetic rows (no group was set up to record from): the shape the statement gives.
#[test]
fn mysql_group_members_synthetic() {
    let mut answers = mysql_answers(fixture!("replication/my84_source"), &ALL_MYSQL);
    let row = |address: &str, state: &str, role: &str, this: &str| {
        vec![Some(address.to_owned()), Some(state.to_owned()), Some(role.to_owned()), Some(this.to_owned())]
    };
    answers[5].1 =
        Answer::Rows(vec![row("gr1:3306", "ONLINE", "PRIMARY", "1"), row("gr2:3306", "RECOVERING", "SECONDARY", "0")]);
    let r = decode::mysql::replication::read(&my(true), &answers).unwrap();
    assert_eq!(r.members.len(), 2);
    assert_eq!(r.role(), Role::Primary, "the group's primary, with its replica connected");
    assert!(r.members[0].this && r.members[0].role.as_deref() == Some("PRIMARY"));
    assert_eq!(r.members[1].state, "RECOVERING");
}
