use super::decode::{self, Answer, DecodeError, Rows};
use super::format::{self, PG_MARKERS};
use super::rate::{self, Gap, History, Pending, Rate};
use super::*;

fn pg(version: u32) -> PgMonitor {
    PgMonitor { version, superuser: true, read_all_stats: true, signal_backend: true, flavour: Flavour::Standard }
}

const IDLE: SessionsWanted = SessionsWanted { idle: true };
const NO_IDLE: SessionsWanted = SessionsWanted { idle: false };

fn load(json: &str) -> Vec<Answer> {
    let value: serde_json::Value = serde_json::from_str(json).expect("fixture is JSON");
    value
        .as_array()
        .expect("a list of statements")
        .iter()
        .map(|statement| match statement {
            serde_json::Value::Array(rows) => Answer::Rows(
                rows.iter()
                    .map(|row| {
                        row.as_array()
                            .expect("a row is a list")
                            .iter()
                            .map(|cell| cell.as_str().map(str::to_owned))
                            .collect()
                    })
                    .collect(),
            ),
            serde_json::Value::Object(error) => Answer::Error {
                code: error["code"].as_u64().map(|c| c as u32),
                message: error["error"].as_str().unwrap_or_default().to_owned(),
            },
            other => panic!("not a statement's answer: {other}"),
        })
        .collect()
}

macro_rules! fixture {
    ($name:literal) => {
        load(include_str!(concat!("fixtures/", $name, ".json")))
    };
}

/// The rows of every statement of a fixture, none refused.
fn rows(answers: Vec<Answer>) -> Vec<Rows> {
    answers
        .into_iter()
        .map(|a| match a {
            Answer::Rows(rows) => rows,
            Answer::Error { message, .. } => panic!("refused: {message}"),
        })
        .collect()
}

fn single(answers: Vec<Answer>) -> Rows {
    let mut all = rows(answers);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

fn by_application<'a>(list: &'a SessionList, name: &str) -> &'a SessionRow {
    list.rows.iter().find(|r| r.application.as_deref() == Some(name)).unwrap_or_else(|| panic!("no {name}"))
}

fn by_id(list: &SessionList, id: i64) -> &SessionRow {
    list.rows.iter().find(|r| r.key.id == id).unwrap_or_else(|| panic!("no session {id}"))
}

fn extra(row: &SessionRow, key: ExtraKey) -> Option<&str> {
    row.extra.iter().find(|e| e.key == key).map(|e| e.value.as_str())
}

#[test]
fn postgres_probe_reads_version_privileges_and_flavour() {
    let p = decode::pg::probe(&single(fixture!("pg17/probe"))).unwrap();
    assert_eq!(p, pg(170011));
    assert!(p.leader_pid());
    let p = decode::pg::probe(&single(fixture!("pg17_unprivileged/probe"))).unwrap();
    assert!(!p.superuser && !p.read_all_stats && !p.signal_backend);
    assert_eq!(decode::pg::probe(&single(fixture!("pg14/probe"))).unwrap().version / 10000, 14);
}

#[test]
fn postgres_too_old_is_said_so() {
    let rows =
        vec![vec![Some("110022".into()), Some("t".into()), Some("t".into()), Some("t".into()), Some(String::new())]];
    assert_eq!(decode::pg::probe(&rows), Err(DecodeError::Unsupported("PostgreSQL 11.0".into())));
}

#[test]
fn provider_markers_name_the_flavour_first_match_first() {
    let pg_with = |names: &[&str]| {
        let row =
            vec![Some("160004".into()), Some("f".into()), Some("t".into()), Some("t".into()), Some(names.join(","))];
        decode::pg::probe(&vec![row]).unwrap().flavour
    };
    assert_eq!(pg_with(&[]), Flavour::Standard);
    assert_eq!(pg_with(&["rds_superuser"]), Flavour::AmazonRds);
    assert_eq!(pg_with(&["aurora_version", "rds_superuser"]), Flavour::AmazonAurora);
    assert_eq!(pg_with(&["cloudsqlsuperuser"]), Flavour::GoogleCloudSql);
    assert_eq!(pg_with(&["alloydbsuperuser", "cloudsqlsuperuser"]), Flavour::GoogleAlloyDb);
    assert_eq!(pg_with(&["azure_pg_admin"]), Flavour::AzureDatabase);

    let my_with = |variables: Vec<Vec<Option<String>>>| {
        let first = Answer::Rows(vec![vec![Some("8.0.35".into()), Some("1".into()), Some("admin@%".into())]]);
        let none = Answer::Rows(Vec::new());
        decode::mysql::probe(&[first, Answer::Rows(variables), none.clone(), none.clone(), none]).unwrap().flavour
    };
    let var = |n: &str, v: &str| vec![Some(n.to_owned()), Some(v.to_owned())];
    assert_eq!(my_with(vec![var("basedir", "/usr/")]), Flavour::Standard);
    assert_eq!(my_with(vec![var("basedir", "/rdsdbbin/mysql-8.0.35.R2/")]), Flavour::AmazonRds);
    assert_eq!(
        my_with(vec![var("aurora_version", "3.05.2"), var("basedir", "/rdsdbbin/oscar-8.0.mysql_aurora.3.05.2/")]),
        Flavour::AmazonAurora
    );
    assert_eq!(Flavour::AmazonRds.product(), Some("Amazon RDS"));
    assert_eq!(Flavour::Standard.product(), None);
    assert_eq!(MonitorFormat::Postgres(pg(170000)).flavour(), Flavour::Standard);
    assert_eq!(format::flavour(PG_MARKERS, &Probe::default()), Flavour::Standard);
}

#[test]
fn postgres_server_info() {
    let info = decode::pg::server_info(&single(fixture!("pg17/server_info"))).unwrap();
    assert!(info.version.starts_with("PostgreSQL 17.11"), "{}", info.version);
    assert!(!info.read_only);
    assert_eq!((info.max_connections, info.cache_bytes), (Some(100), Some(134_217_728)));
    assert!(info.started.unwrap() < info.read_at);
    assert_eq!(info.extra, [Extra { key: ExtraKey::TrackIoTiming, value: "off".into() }]);
    decode::pg::server_info(&single(fixture!("pg14/server_info"))).unwrap();
}

#[test]
fn postgres_counters_and_session_counts() {
    let own = Own { statements: 0, transactions: 9 };
    let (sample, gauges) = decode::pg::counters(&single(fixture!("pg17_locks/counters")), own).unwrap();
    assert_eq!(sample.get(CounterKind::Commits), Some(6870));
    assert_eq!(sample.get(CounterKind::PageReads), Some(1_778_632));
    assert_eq!(sample.get(CounterKind::PageRequests), Some(1_778_632 + 727_627));
    assert_eq!(sample.get(CounterKind::RowsWritten), Some(23));
    assert_eq!(sample.get(CounterKind::Statements), None, "PostgreSQL counts no statements");
    assert_eq!(sample.epoch, Epoch::Mark(0, 5), "never reset, five databases");
    assert_eq!(sample.own, own);
    let gauge = |k| gauges.iter().find(|g| g.kind == k).map(|g| g.value);
    // Four waiting or sleeping statements and the lock holder idle in its transaction; the
    // monitor's own session is not counted as active.
    assert_eq!(gauge(GaugeKind::Sessions), Some(7));
    assert_eq!(gauge(GaugeKind::Active), Some(5));
    assert_eq!(gauge(GaugeKind::IdleInTx), Some(1));
    assert_eq!(gauge(GaugeKind::WaitingForLock), Some(3));
    assert_eq!(gauge(GaugeKind::Hidden), Some(0));
    let (_, gauges) = decode::pg::counters(&single(fixture!("pg17_unprivileged/counters")), own).unwrap();
    let gauge = |k| gauges.iter().find(|g| g.kind == k).map(|g| g.value);
    assert_eq!((gauge(GaugeKind::Sessions), gauge(GaugeKind::Hidden)), (Some(1), Some(11)));
    decode::pg::counters(&single(fixture!("pg14/counters")), own).unwrap();
}

#[test]
fn postgres_sessions_show_who_waits_for_whom() {
    let (list, hidden) = decode::pg::sessions(&single(fixture!("pg17_locks/sessions")), IDLE).unwrap();
    assert!(hidden.is_empty());
    assert!(!list.idle_left_out);
    let holder = by_application(&list, "zz_mon_a");
    assert_eq!(holder.state, SessionState::IdleInTx);
    assert!(matches!(holder.tx, Tx::Since(_)));
    assert_eq!(holder.wait, Some(Wait { class: WaitClass::Client, name: "ClientRead".into() }));
    assert_eq!(holder.blocked_by, None, "not waiting for a lock: not asked");
    assert_eq!(holder.client.as_deref(), Some("127.0.0.1"));
    assert!(holder.key.started.is_some());
    let update = by_application(&list, "zz_mon_b");
    assert!(update.waits_for_lock());
    assert_eq!(update.blocked_by, Some(vec![Blocker::Session(holder.key.id)]));
    let alter = by_application(&list, "zz_mon_c");
    assert_eq!(alter.blocked_by, Some(vec![Blocker::Session(holder.key.id), Blocker::Session(update.key.id)]));
    let select = by_application(&list, "zz_mon_d");
    assert_eq!(select.blocked_by, Some(vec![Blocker::Session(alter.key.id)]));
    assert_eq!(select.wait.as_ref().map(|w| w.name.as_str()), Some("relation"));
    // 300 ASCII characters: shortened by the list, whole on the server.
    let long = by_application(&list, "zz_mon_e");
    let q = long.query.as_ref().unwrap();
    assert!(q.shortened && !q.maybe_cut_by_server);
    assert_eq!(q.text.chars().count(), QUERY_CHARS);
    assert_eq!(long.wait, Some(Wait { class: WaitClass::Timeout, name: "PgSleep".into() }));
    // Over the server's 1 kB of two-byte characters: maybe cut by the server too.
    let q = by_application(&list, "zz_mon_f").query.as_ref().unwrap();
    assert!(q.shortened && q.maybe_cut_by_server);
    assert_eq!(q.text.chars().count(), QUERY_CHARS);
    let monitor: Vec<_> = list.rows.iter().filter(|r| r.monitor).collect();
    assert_eq!(monitor.len(), 1);
    assert_eq!(monitor[0].state, SessionState::Active);
    let checkpointer = list.rows.iter().find(|r| r.kind == SessionKind::Background("checkpointer".into())).unwrap();
    assert_eq!(checkpointer.state, SessionState::Background);
    assert_eq!((checkpointer.query.as_ref(), checkpointer.tx), (None, Tx::None));
    assert!(list.rows.windows(2).all(|w| w[0].key.id < w[1].key.id));
    assert!(list.rows.iter().all(|r| r.visible == Visibility::Full));

    let (list, _) = decode::pg::sessions(&single(fixture!("pg17_locks/sessions_without_idle")), NO_IDLE).unwrap();
    assert!(list.idle_left_out);
    let (list, _) = decode::pg::sessions(&single(fixture!("pg14/sessions")), IDLE).unwrap();
    assert!(list.rows.iter().any(|r| r.monitor));
}

#[test]
fn postgres_hidden_sessions_are_unknown_not_idle() {
    let (list, hidden) = decode::pg::sessions(&single(fixture!("pg17_unprivileged/sessions")), IDLE).unwrap();
    assert_eq!(hidden, [Hidden { part: Part::OtherSessionDetails, need: Need::Grant("pg_read_all_stats") }]);
    let other = by_application(&list, "zz_mon_a");
    assert_eq!(other.visible, Visibility::Hidden);
    assert_eq!((&other.kind, &other.state, other.tx), (&SessionKind::Unknown, &SessionState::Unknown, Tx::Unknown));
    assert_eq!((other.query.as_ref(), other.wait.as_ref(), other.blocked_by.as_ref()), (None, None, None));
    assert_eq!(other.user.as_deref(), Some("datarig"), "the role is still shown");
    let own = list.rows.iter().find(|r| r.monitor).unwrap();
    assert_eq!((own.visible, own.user.as_deref()), (Visibility::Full, Some("zz_mon_role")));
    assert_eq!(list.rows.iter().filter(|r| r.visible == Visibility::Hidden).count(), 11);
}

#[test]
fn postgres_blockers_read_prepared_transactions() {
    let row = |blocked: &str| {
        let mut cells: Vec<Option<String>> = vec![None; 21];
        for (i, v) in [(0, "1"), (1, "7"), (8, "client backend"), (9, "active"), (10, "Lock"), (11, "tuple"), (19, "f")]
        {
            cells[i] = Some(v.into());
        }
        cells[20] = Some(blocked.into());
        cells
    };
    let (list, _) = decode::pg::sessions(&vec![row("{0,12}")], IDLE).unwrap();
    assert_eq!(list.rows[0].blocked_by, Some(vec![Blocker::PreparedTransaction, Blocker::Session(12)]));
    let (list, _) = decode::pg::sessions(&vec![row("{}")], IDLE).unwrap();
    assert_eq!(list.rows[0].blocked_by, Some(Vec::new()));
    assert!(matches!(decode::pg::sessions(&vec![row("12")], IDLE), Err(DecodeError::Malformed { .. })));
}

#[test]
fn malformed_rows_are_errors_never_defaults() {
    let mut rows = single(fixture!("pg17/counters"));
    rows[0][1] = Some("lots".into());
    assert!(matches!(decode::pg::counters(&rows, Own::default()), Err(DecodeError::Malformed { .. })));
    rows[0].pop();
    assert!(matches!(decode::pg::counters(&rows, Own::default()), Err(DecodeError::Malformed { .. })));
    assert!(matches!(decode::pg::counters(&Vec::new(), Own::default()), Err(DecodeError::Malformed { .. })));
    let mut rows = single(fixture!("pg17/sessions"));
    rows[1][0] = Some("1".into());
    let err = decode::pg::sessions(&rows, IDLE).unwrap_err();
    assert_eq!(err.to_string(), "unexpected monitor rows (sessions): rows of different snapshots");
    let mut rows = single(fixture!("pg17/sessions"));
    rows[0][19] = None;
    assert!(decode::pg::sessions(&rows, IDLE).is_err());
    let mut status = rows_of_my_counters();
    status[1].retain(|r| r[0].as_deref() != Some("Uptime"));
    assert!(matches!(decode::mysql::counters(&status, Own::default()), Err(DecodeError::Malformed { .. })));
}

fn rows_of_my_counters() -> Vec<Rows> {
    rows(fixture!("my84_root/counters"))
}

#[test]
fn mysql_probe_reads_what_the_account_may() {
    let p = decode::mysql::probe(&fixture!("my84_unprivileged/probe")).unwrap();
    assert_eq!(p.version, 80411);
    assert!(p.performance_schema && p.ps_processlist());
    assert_eq!(p.user, "datarig");
    assert_eq!(p.process, Readable::Unknown, "no direct grant seen: cannot tell");
    assert_eq!((p.innodb_trx, p.metadata_locks), (Readable::Refused, Readable::Refused));
    assert_eq!(p.flavour, Flavour::Standard);
    let p = decode::mysql::probe(&fixture!("my84_root/probe")).unwrap();
    assert_eq!((p.user.as_str(), p.process, p.innodb_trx), ("root", Readable::Yes, Readable::Yes));
    assert_eq!(p.metadata_locks, Readable::Yes);
    let p = decode::mysql::probe(&fixture!("my80/probe")).unwrap();
    assert_eq!((p.version, p.innodb_trx), (80045, Readable::Yes));
}

#[test]
fn mysql_versions_the_monitor_does_not_read() {
    let probe = |version: &str| {
        let first = Answer::Rows(vec![vec![Some(version.into()), Some("1".into()), Some("u@%".into())]]);
        let none = Answer::Rows(Vec::new());
        decode::mysql::probe(&[first, none.clone(), none.clone(), none.clone(), none]).map(|m| m.version)
    };
    assert_eq!(probe("8.0.35-0ubuntu0.22.04.1"), Ok(80035));
    assert_eq!(probe("8.0.28-log"), Ok(80028));
    assert_eq!(probe("8.0.17"), Ok(80017));
    for old in ["8.0.16", "5.7.44-log", "10.11.6-MariaDB", "8.0.mysql_aurora.3.05.2", "eight"] {
        assert!(matches!(probe(old), Err(DecodeError::Unsupported(_))), "{old}");
    }
    let failed = Answer::Error { code: Some(2013), message: "Lost connection".into() };
    assert_eq!(
        decode::mysql::probe(&[failed]),
        Err(DecodeError::Malformed { what: "probe", detail: "1 answers, expected 5".into() })
    );
    let failed = Answer::Error { code: Some(2013), message: "Lost connection".into() };
    let none = Answer::Rows(Vec::new());
    assert_eq!(
        decode::mysql::probe(&[failed, none.clone(), none.clone(), none.clone(), none]),
        Err(DecodeError::ProbeFailed("Lost connection".into()))
    );
    // A refusal for another reason than a privilege cannot tell.
    let first = Answer::Rows(vec![vec![Some("8.4.0".into()), Some("0".into()), Some("u@%".into())]]);
    let none = Answer::Rows(Vec::new());
    let other = Answer::Error { code: Some(1030), message: "storage engine".into() };
    let p = decode::mysql::probe(&[first, none.clone(), none.clone(), other.clone(), other]).unwrap();
    assert_eq!((p.innodb_trx, p.metadata_locks), (Readable::Unknown, Readable::Unknown));
    assert!(!p.ps_processlist(), "performance schema off");
}

#[test]
fn mysql_server_info() {
    let info = decode::mysql::server_info(&rows(fixture!("my84_unprivileged/server_info"))).unwrap();
    assert_eq!(info.version, "8.4.11 MySQL Community Server - GPL");
    assert_eq!(info.started, Some(Timestamp(info.read_at.0 - 191_331 * 1_000_000)));
    assert!(!info.read_only);
    assert_eq!((info.max_connections, info.cache_bytes), (Some(151), Some(134_217_728)));
    let gtid = info.extra.iter().find(|e| e.key == ExtraKey::GtidMode).unwrap();
    assert_eq!((gtid.key.name(), gtid.value.as_str()), ("gtid_mode", "OFF"));
    let info = decode::mysql::server_info(&rows(fixture!("my80/server_info"))).unwrap();
    assert!(info.version.starts_with("8.0.45"));
}

#[test]
fn mysql_counters() {
    let own = Own { statements: 40, transactions: 0 };
    let (sample, gauges) = decode::mysql::counters(&rows_of_my_counters(), own).unwrap();
    assert_eq!(sample.epoch, Epoch::Uptime(191_332));
    assert_eq!(sample.get(CounterKind::Statements), Some(64_275));
    assert_eq!(sample.get(CounterKind::ExplicitRollbacks), Some(2));
    assert_eq!(sample.get(CounterKind::Commits), None, "MySQL counts no transactions");
    assert_eq!(sample.get(CounterKind::RowsWritten), Some(16));
    assert_eq!(sample.get(CounterKind::RowLockWaits), Some(4));
    let gauge = |k| gauges.iter().find(|g| g.kind == k).map(|g| g.value);
    assert_eq!((gauge(GaugeKind::Sessions), gauge(GaugeKind::Active)), (Some(7), Some(4)));
    assert_eq!(gauge(GaugeKind::IdleInTx), None, "MySQL has no such counter");
    decode::mysql::counters(&rows(fixture!("my80/counters")), own).unwrap();
    decode::mysql::counters(&rows(fixture!("my84_unprivileged/counters")), own).unwrap();
}

#[test]
fn mysql_sessions_with_transactions() {
    let m = decode::mysql::probe(&fixture!("my84_root/probe")).unwrap();
    let (list, hidden) = decode::mysql::sessions(&rows(fixture!("my84_root/sessions")), &m, IDLE).unwrap();
    assert!(hidden.is_empty());
    assert!(!list.idle_left_out);
    let holder = by_id(&list, 63113);
    assert_eq!(holder.state, SessionState::IdleInTx);
    assert_eq!(holder.tx, Tx::Since(Timestamp(1_794_068_765_000_000)));
    assert_eq!(holder.query, None);
    assert_eq!(extra(holder, ExtraKey::RowsLocked), Some("1"));
    let update = by_id(&list, 63114);
    assert_eq!(
        (update.state.clone(), update.wait.clone()),
        (SessionState::Active, Some(Wait { class: WaitClass::Lock, name: "row lock".into() }))
    );
    assert_eq!(update.query_started, Some(Timestamp(list.time.0 - 5_000_000)));
    assert_eq!(extra(update, ExtraKey::State), Some("updating"));
    // A transaction that read only a MyISAM table, and LOCK TABLES: no InnoDB transaction, but
    // locks held until the transaction ends.
    for id in [63116, 63117] {
        let idle = by_id(&list, id);
        assert_eq!((&idle.state, idle.tx), (&SessionState::IdleInTx, Tx::Open), "{id}");
    }
    let alter = by_id(&list, 63118);
    assert_eq!(alter.wait, Some(Wait { class: WaitClass::Lock, name: "table metadata lock".into() }));
    let sleep = by_id(&list, 63119);
    assert!(sleep.query.as_ref().unwrap().shortened && sleep.wait.is_none());
    assert_eq!(sleep.tx, Tx::None, "no transaction and no lock past its statement");
    let daemon = by_id(&list, 5);
    assert_eq!(
        (&daemon.kind, &daemon.state),
        (&SessionKind::Background("event_scheduler".into()), &SessionState::Background)
    );
    assert_eq!(daemon.database, None);
    assert_eq!(list.rows.iter().filter(|r| r.monitor).count(), 1);
    let (list, _) = decode::mysql::sessions(&rows(fixture!("my84_root/sessions_without_idle")), &m, NO_IDLE).unwrap();
    assert!(list.idle_left_out);
    assert!(list.rows.iter().any(|r| r.key.id == 63116), "an idle transaction holding locks is kept");
    let m = decode::mysql::probe(&fixture!("my80/probe")).unwrap();
    decode::mysql::sessions(&rows(fixture!("my80/sessions")), &m, IDLE).unwrap();
}

#[test]
fn mysql_sessions_without_process_say_what_is_missing() {
    let m = decode::mysql::probe(&fixture!("my84_unprivileged/probe")).unwrap();
    let (list, hidden) = decode::mysql::sessions(&rows(fixture!("my84_unprivileged/sessions")), &m, NO_IDLE).unwrap();
    assert_eq!(
        hidden,
        [
            Hidden { part: Part::OtherSessions, need: Need::Grant("PROCESS") },
            Hidden { part: Part::TransactionState, need: Need::Grant("PROCESS") },
        ]
    );
    // Idle sessions cannot be left out when their transactions are unknown.
    assert!(!list.idle_left_out);
    let sleeping = by_id(&list, 63113);
    assert_eq!((&sleeping.state, sleeping.tx), (&SessionState::Idle, Tx::Unknown));
    assert_eq!(extra(sleeping, ExtraKey::State), None, "an empty STATE is none");
    assert_eq!(by_id(&list, 63114).wait, None, "a row lock wait is not known without INNODB_TRX");
    assert!(by_id(&list, 63118).waits_for_lock());
    assert_eq!(by_id(&list, 63116).tx, Tx::Unknown, "neither its InnoDB transaction nor its locks are known");
    // The same account with PROCESS through a role lists others: no note then.
    let mut m = m;
    m.user = "someone_else".into();
    let (_, hidden) = decode::mysql::sessions(&rows(fixture!("my84_unprivileged/sessions")), &m, NO_IDLE).unwrap();
    assert_eq!(hidden, [Hidden { part: Part::TransactionState, need: Need::Grant("PROCESS") }]);
}

fn sample(time: i64, epoch: Epoch, counters: &[(CounterKind, u64)], own: Own) -> Sample {
    Sample {
        time: Timestamp(time * 1_000_000),
        epoch,
        counters: counters.iter().map(|&(kind, value)| Counter { kind, value }).collect(),
        own,
    }
}

#[test]
fn rates_take_out_the_monitors_own_traffic() {
    let own = |statements, transactions| Own { statements, transactions };
    let a = sample(100, Epoch::Uptime(50), &[(CounterKind::Statements, 1000), (CounterKind::RowsRead, 10)], own(7, 0));
    let b = sample(102, Epoch::Uptime(52), &[(CounterKind::Statements, 1010), (CounterKind::RowsRead, 30)], own(10, 0));
    // Ten statements in two seconds, three of them ours.
    assert_eq!(rate::rate(&a, &b, CounterKind::Statements, &mut Pending::default()), Some(Rate::PerSecond(3.5)));
    assert_eq!(rate::rate(&a, &b, CounterKind::RowsRead, &mut Pending::default()), Some(Rate::PerSecond(10.0)));
    assert_eq!(rate::rate(&a, &b, CounterKind::Commits, &mut Pending::default()), None);
    // More of ours than the server counted yet: nothing, never below, and the rest is carried.
    let c = sample(104, Epoch::Uptime(54), &[(CounterKind::Statements, 1012)], own(20, 0));
    let mut pending = Pending::default();
    assert_eq!(rate::rate(&b, &c, CounterKind::Statements, &mut pending), Some(Rate::PerSecond(0.0)));
    assert_eq!(pending, Pending { statements: 8, transactions: 0 });
    let pa = sample(10, Epoch::Mark(0, 5), &[(CounterKind::Commits, 100)], own(0, 4));
    let pb = sample(11, Epoch::Mark(0, 5), &[(CounterKind::Commits, 106)], own(0, 5));
    assert_eq!(rate::rate(&pa, &pb, CounterKind::Commits, &mut Pending::default()), Some(Rate::PerSecond(5.0)));
}

#[test]
fn rates_never_span_a_reset() {
    let n = Own::default();
    let k = CounterKind::RowsRead;
    let up = |t, u, v| sample(t, Epoch::Uptime(u), &[(k, v)], n);
    assert_eq!(rate::rate(&up(0, 100, 5), &up(2, 102, 9), k, &mut Pending::default()), Some(Rate::PerSecond(2.0)));
    assert_eq!(
        rate::rate(&up(0, 100, 5), &up(2, 1, 9), k, &mut Pending::default()),
        Some(Rate::Gap(Gap::Reset)),
        "restarted"
    );
    assert_eq!(rate::rate(&up(0, 100, 5), &up(2, 102, 4), k, &mut Pending::default()), Some(Rate::Gap(Gap::Backwards)));
    assert_eq!(rate::rate(&up(2, 100, 5), &up(2, 100, 9), k, &mut Pending::default()), Some(Rate::Gap(Gap::NoTime)));
    assert_eq!(rate::rate(&up(3, 100, 5), &up(2, 100, 9), k, &mut Pending::default()), Some(Rate::Gap(Gap::NoTime)));
    let mark = |t, reset, dbs, v| sample(t, Epoch::Mark(reset, dbs), &[(k, v)], n);
    assert_eq!(
        rate::rate(&mark(0, 0, 5, 5), &mark(1, 0, 5, 6), k, &mut Pending::default()),
        Some(Rate::PerSecond(1.0))
    );
    assert_eq!(
        rate::rate(&mark(0, 0, 5, 5), &mark(1, 77, 5, 9), k, &mut Pending::default()),
        Some(Rate::Gap(Gap::Reset)),
        "reset"
    );
    assert_eq!(
        rate::rate(&mark(0, 0, 5, 5), &mark(1, 0, 4, 9), k, &mut Pending::default()),
        Some(Rate::Gap(Gap::Reset)),
        "dropped"
    );
    assert_eq!(rate::rate(&mark(0, 0, 5, 5), &up(1, 9, 9), k, &mut Pending::default()), Some(Rate::Gap(Gap::Reset)));
    // A new monitor connection counts its own from nothing again.
    let s = CounterKind::Statements;
    let a = sample(0, Epoch::Uptime(9), &[(s, 50)], Own { statements: 30, transactions: 0 });
    let b = sample(2, Epoch::Uptime(11), &[(s, 60)], Own { statements: 2, transactions: 0 });
    assert_eq!(rate::rate(&a, &b, s, &mut Pending::default()), Some(Rate::Gap(Gap::Reconnected)));
}

#[test]
fn cache_hit_is_the_share_of_pages_found() {
    let n = Own::default();
    let s = |t, requests, reads| {
        sample(t, Epoch::Uptime(t as u64), &[(CounterKind::PageRequests, requests), (CounterKind::PageReads, reads)], n)
    };
    assert_eq!(rate::cache_hit(&s(0, 100, 10), &s(1, 300, 30)), Some(Ok(0.9)));
    assert_eq!(rate::cache_hit(&s(0, 100, 10), &s(1, 100, 10)), None, "nothing asked: no ratio");
    assert_eq!(rate::cache_hit(&s(5, 100, 10), &s(1, 300, 30)), Some(Err(Gap::Reset)));
    assert_eq!(rate::cache_hit(&s(0, 100, 10), &s(1, 300, 5)), Some(Err(Gap::Backwards)));
    let none = sample(1, Epoch::Uptime(1), &[], n);
    assert_eq!(rate::cache_hit(&none, &none), None);
}

#[test]
fn history_is_bounded_and_in_clock_order() {
    let mut h = History::new(3);
    assert!(h.is_empty() && h.last().is_none());
    for (t, v) in [(1, Some(1.0)), (2, None), (3, Some(3.0)), (4, Some(4.0))] {
        assert!(h.push(Timestamp(t), v));
    }
    assert_eq!(h.len(), 3);
    assert_eq!(
        h.points().collect::<Vec<_>>(),
        [(Timestamp(2), None), (Timestamp(3), Some(3.0)), (Timestamp(4), Some(4.0))]
    );
    assert!(!h.push(Timestamp(4), Some(9.0)), "not after the last: refused");
    assert_eq!(h.last(), Some(4.0));
    assert!(h.push(Timestamp(5), None));
    assert_eq!(h.last(), None, "a gap is no value");
    assert_eq!(Timestamp(5).since(Timestamp(7)), None);
}

#[test]
fn own_transactions_published_late_are_still_taken_out() {
    // PostgreSQL publishes the monitor's commits when it flushes its statistics: two ticks of
    // ours show only in the third sample. Both are ours, so no load at all.
    let at = |t, commits, own| {
        sample(t, Epoch::Mark(0, 1), &[(CounterKind::Commits, commits)], Own { statements: 0, transactions: own })
    };
    let (a, b, c) = (at(0, 100, 10), at(2, 100, 11), at(4, 102, 12));
    let mut pending = Pending::default();
    assert_eq!(rate::rate(&a, &b, CounterKind::Commits, &mut pending), Some(Rate::PerSecond(0.0)));
    assert_eq!(pending.transactions, 1);
    assert_eq!(rate::rate(&b, &c, CounterKind::Commits, &mut pending), Some(Rate::PerSecond(0.0)));
    assert_eq!(pending.transactions, 0);
    // Real load drains what is carried and shows the rest.
    let d = at(6, 112, 13);
    pending.transactions = 3;
    assert_eq!(rate::rate(&c, &d, CounterKind::Commits, &mut pending), Some(Rate::PerSecond(3.0)));
    assert_eq!(pending.transactions, 0);
    // Across a gap nothing is carried.
    pending.transactions = 5;
    let reset = sample(8, Epoch::Mark(9, 1), &[(CounterKind::Commits, 1)], Own { statements: 0, transactions: 14 });
    assert_eq!(rate::rate(&d, &reset, CounterKind::Commits, &mut pending), Some(Rate::Gap(Gap::Reset)));
    assert_eq!(pending.transactions, 0);
    // A counter the monitor does not count leaves the carry alone.
    pending.statements = 2;
    let rows = |t, v| sample(t, Epoch::Uptime(t as u64), &[(CounterKind::RowsRead, v)], Own::default());
    assert_eq!(rate::rate(&rows(1, 0), &rows(2, 4), CounterKind::RowsRead, &mut pending), Some(Rate::PerSecond(4.0)));
    assert_eq!(pending.statements, 2);
}

#[test]
fn super_without_process_does_not_show_other_sessions() {
    let first = Answer::Rows(vec![vec![Some("8.4.0".into()), Some("1".into()), Some("ops@%".into())]]);
    let grants = |rows: Vec<&str>| Answer::Rows(rows.into_iter().map(|g| vec![Some(g.to_owned())]).collect());
    let none = Answer::Rows(Vec::new());
    let with = |g| decode::mysql::probe(&[first.clone(), none.clone(), g, none.clone(), none.clone()]).unwrap().process;
    assert_eq!(with(grants(vec!["PROCESS"])), Readable::Yes);
    assert_eq!(with(grants(vec!["SUPER"])), Readable::Unknown);
    assert_eq!(with(grants(vec![])), Readable::Unknown);
}

#[test]
fn a_postgres_tick_reads_what_is_wanted_in_order() {
    let limit = std::time::Duration::from_secs(2);
    let counters_only = Want { sessions: None, server: false, limit };
    let all = Want { sessions: Some(NO_IDLE), server: true, limit };
    let own = Own { statements: 0, transactions: 3 };
    let rows = vec![
        single(fixture!("pg17_locks/counters")),
        single(fixture!("pg17_unprivileged/sessions")),
        single(fixture!("pg17/server_info")),
    ];
    let s = decode::pg::tick(&all, &rows, own).unwrap();
    assert_eq!(s.sample.own, own);
    assert!(s.sessions.as_ref().unwrap().idle_left_out);
    assert_eq!(s.hidden.len(), 1);
    assert!(s.server.is_some());
    let s = decode::pg::tick(&counters_only, &rows[..1], own).unwrap();
    assert_eq!((s.sessions, s.server, s.hidden.len()), (None, None, 0));
    assert!(matches!(decode::pg::tick(&all, &rows[..2], own), Err(DecodeError::Malformed { .. })));
}

#[test]
fn decode_errors_become_session_errors() {
    use crate::driver::DbError;
    assert_eq!(
        DecodeError::Unsupported("PostgreSQL 11.0".into()).into_db(format::PG_OLDEST),
        DbError::VersionUnsupported { server: "PostgreSQL 11.0".into(), needed: "PostgreSQL 12".into() }
    );
    assert_eq!(
        DecodeError::ProbeFailed("denied".into()).into_db(format::MYSQL_OLDEST),
        DbError::Server("denied".into())
    );
    let malformed = DecodeError::Malformed { what: "tick", detail: "1 statements, expected 2".into() };
    assert!(matches!(malformed.into_db(format::PG_OLDEST), DbError::Connection(_)));
}

#[test]
fn a_session_without_activity_tracking_has_an_unknown_transaction() {
    let mut cells: Vec<Option<String>> = vec![None; 21];
    for (i, v) in [(0, "1"), (1, "9"), (3, "app"), (8, "client backend"), (9, "disabled"), (16, ""), (19, "f")] {
        cells[i] = Some(v.into());
    }
    let (list, hidden) = decode::pg::sessions(&vec![cells], IDLE).unwrap();
    let row = &list.rows[0];
    assert_eq!(
        (&row.state, row.tx, row.visible),
        (&SessionState::Other("disabled".into()), Tx::Unknown, Visibility::Full)
    );
    assert!(hidden.is_empty());
}

#[test]
fn a_mysql_tick_reads_what_is_wanted_in_order() {
    let m = decode::mysql::probe(&fixture!("my84_root/probe")).unwrap();
    let limit = std::time::Duration::from_secs(2);
    let counters_only = Want { sessions: None, server: false, limit };
    let all = Want { sessions: Some(IDLE), server: true, limit };
    let own = Own { statements: 12, transactions: 0 };
    let mut rows = rows(fixture!("my84_root/counters"));
    rows.extend(rows_of_sessions());
    rows.extend(rows_of_server());
    let s = decode::mysql::tick(&m, &all, &rows, own).unwrap();
    assert_eq!(s.sample.own, own);
    assert_eq!(s.sessions.as_ref().map(|l| l.rows.len()), Some(8));
    assert!(s.server.is_some() && s.hidden.is_empty());
    let s = decode::mysql::tick(&m, &counters_only, &rows[..2], own).unwrap();
    assert_eq!((s.sessions, s.server), (None, None));
    assert!(matches!(decode::mysql::tick(&m, &all, &rows[..3], own), Err(DecodeError::Malformed { .. })));
}

fn rows_of_sessions() -> Vec<Rows> {
    rows(fixture!("my84_root/sessions"))
}

fn rows_of_server() -> Vec<Rows> {
    rows(fixture!("my84_unprivileged/server_info"))
}

#[test]
fn sessions_performance_schema_could_not_track_are_said_to_be_missing() {
    let m = decode::mysql::probe(&fixture!("my84_root/probe")).unwrap();
    let want = Want { sessions: Some(IDLE), server: false, limit: std::time::Duration::from_secs(1) };
    let mut rows = rows(fixture!("my84_root/counters"));
    rows.extend(rows_of_sessions());
    let lost = |rows: &mut Vec<Rows>, n: &str| {
        for r in rows[1].iter_mut().filter(|r| r[0].as_deref() == Some("Performance_schema_thread_instances_lost")) {
            r[1] = Some(n.into());
        }
    };
    assert!(decode::mysql::tick(&m, &want, &rows, Own::default()).unwrap().hidden.is_empty());
    lost(&mut rows, "3");
    let untracked =
        Hidden { part: Part::UntrackedSessions, need: Need::Setting("performance_schema_max_thread_instances") };
    assert_eq!(decode::mysql::tick(&m, &want, &rows, Own::default()).unwrap().hidden, [untracked]);
    // The list from information_schema does not depend on it.
    let old = MySqlMonitor { version: 80021, ..m };
    assert!(decode::mysql::tick(&old, &want, &rows, Own::default()).unwrap().hidden.is_empty());
}
#[test]
fn a_session_answers_a_monitor_tick_it_does_not_do() {
    use crate::driver::{DbCommand, DbError, DbEvent};
    let want = Want { sessions: None, server: false, limit: std::time::Duration::from_secs(1) };
    match (DbCommand::MonitorTick { request: 7, want }).not_supported() {
        DbEvent::MonitorSnapshot { request: 7, result: Err(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
    match (DbCommand::FetchMore { id: 3 }).not_supported() {
        DbEvent::Failed { id: 3, error: DbError::NotSupported, cancelled: false } => {}
        other => panic!("{other:?}"),
    }
}
