use super::decode::{self, Answer, DecodeError, Rows};
use super::format::{self, PG_MARKERS};
use super::rate::{self, Gap, History, Pending, Rate};
use super::*;

fn pg(version: u32) -> PgMonitor {
    PgMonitor {
        version,
        superuser: true,
        read_all_stats: true,
        signal_backend: true,
        flavour: Flavour::Standard,
        route: Route::Unknown,
    }
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
        decode::mysql::probe(&[first, Answer::Rows(variables), none.clone(), none.clone(), none.clone(), none])
            .unwrap()
            .flavour
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
    assert_eq!(
        (p.innodb_trx, p.metadata_locks, p.statements),
        (Readable::Refused, Readable::Refused, Readable::Refused)
    );
    assert_eq!(p.flavour, Flavour::Standard);
    let p = decode::mysql::probe(&fixture!("my84_root/probe")).unwrap();
    assert_eq!((p.user.as_str(), p.process, p.innodb_trx), ("root", Readable::Yes, Readable::Yes));
    assert_eq!((p.metadata_locks, p.statements), (Readable::Yes, Readable::Yes));
    let p = decode::mysql::probe(&fixture!("my80/probe")).unwrap();
    assert_eq!((p.version, p.innodb_trx), (80045, Readable::Yes));
}

#[test]
fn mysql_versions_the_monitor_does_not_read() {
    let probe = |version: &str| {
        let first = Answer::Rows(vec![vec![Some(version.into()), Some("1".into()), Some("u@%".into())]]);
        let none = Answer::Rows(Vec::new());
        decode::mysql::probe(&[first, none.clone(), none.clone(), none.clone(), none.clone(), none]).map(|m| m.version)
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
        Err(DecodeError::Malformed { what: "probe", detail: "1 answers, expected 6".into() })
    );
    let failed = Answer::Error { code: Some(2013), message: "Lost connection".into() };
    let none = Answer::Rows(Vec::new());
    assert_eq!(
        decode::mysql::probe(&[failed, none.clone(), none.clone(), none.clone(), none.clone(), none]),
        Err(DecodeError::ProbeFailed("Lost connection".into()))
    );
    // A refusal for another reason than a privilege cannot tell.
    let first = Answer::Rows(vec![vec![Some("8.4.0".into()), Some("0".into()), Some("u@%".into())]]);
    let none = Answer::Rows(Vec::new());
    let other = Answer::Error { code: Some(1030), message: "storage engine".into() };
    let p = decode::mysql::probe(&[first, none.clone(), none.clone(), other.clone(), other.clone(), other]).unwrap();
    assert_eq!(
        (p.innodb_trx, p.metadata_locks, p.statements),
        (Readable::Unknown, Readable::Unknown, Readable::Unknown)
    );
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
    let with = |g| {
        decode::mysql::probe(&[first.clone(), none.clone(), g, none.clone(), none.clone(), none.clone()])
            .unwrap()
            .process
    };
    assert_eq!(with(grants(vec!["PROCESS"])), Readable::Yes);
    assert_eq!(with(grants(vec!["SUPER"])), Readable::Unknown);
    assert_eq!(with(grants(vec![])), Readable::Unknown);
}

#[test]
fn a_postgres_tick_reads_what_is_wanted_in_order() {
    let limit = std::time::Duration::from_secs(2);
    let counters_only = Want { sessions: None, locks: false, server: false, limit };
    let all = Want { sessions: Some(NO_IDLE), locks: false, server: true, limit };
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
    let counters_only = Want { sessions: None, locks: false, server: false, limit };
    let all = Want { sessions: Some(IDLE), locks: false, server: true, limit };
    let own = Own { statements: 12, transactions: 0 };
    let mut rows = rows(fixture!("my84_root/counters"));
    rows.extend(rows_of_sessions());
    rows.extend(rows_of_server());
    let s = decode::mysql::tick(&m, &all, &rows, None, own).unwrap();
    assert_eq!(s.sample.own, own);
    assert_eq!(s.sessions.as_ref().map(|l| l.rows.len()), Some(8));
    assert!(s.server.is_some() && s.hidden.is_empty());
    let s = decode::mysql::tick(&m, &counters_only, &rows[..2], None, own).unwrap();
    assert_eq!((s.sessions, s.server), (None, None));
    assert!(matches!(decode::mysql::tick(&m, &all, &rows[..3], None, own), Err(DecodeError::Malformed { .. })));
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
    let want = Want { sessions: Some(IDLE), locks: false, server: false, limit: std::time::Duration::from_secs(1) };
    let mut rows = rows(fixture!("my84_root/counters"));
    rows.extend(rows_of_sessions());
    let lost = |rows: &mut Vec<Rows>, n: &str| {
        for r in rows[1].iter_mut().filter(|r| r[0].as_deref() == Some("Performance_schema_thread_instances_lost")) {
            r[1] = Some(n.into());
        }
    };
    assert!(decode::mysql::tick(&m, &want, &rows, None, Own::default()).unwrap().hidden.is_empty());
    lost(&mut rows, "3");
    let untracked =
        Hidden { part: Part::UntrackedSessions, need: Need::Setting("performance_schema_max_thread_instances") };
    assert_eq!(decode::mysql::tick(&m, &want, &rows, None, Own::default()).unwrap().hidden, [untracked]);
    // The list from information_schema does not depend on it.
    let old = MySqlMonitor { version: 80021, ..m };
    assert!(decode::mysql::tick(&old, &want, &rows, None, Own::default()).unwrap().hidden.is_empty());
}
#[test]
fn a_session_answers_a_monitor_tick_it_does_not_do() {
    use crate::driver::{DbCommand, DbError, DbEvent};
    let want = Want { sessions: None, locks: false, server: false, limit: std::time::Duration::from_secs(1) };
    match (DbCommand::MonitorTick { request: 7, want }).not_supported() {
        DbEvent::MonitorSnapshot { request: 7, result: Err(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
    match (DbCommand::FetchMore { id: 3 }).not_supported() {
        DbEvent::Failed { id: 3, error: DbError::NotSupported, cancelled: false } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_session_answers_a_text_read_it_does_not_do() {
    use crate::driver::{DbCommand, DbError, DbEvent};
    let ask = TextAsk {
        session: SessionKey { id: 1, started: None },
        query_started: None,
        start: String::new(),
        seen: Timestamp(0),
    };
    match (DbCommand::MonitorQueryText { request: 9, ask }).not_supported() {
        DbEvent::MonitorQueryText { request: 9, result: Err(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
}

/// A row read for a PostgreSQL statement's whole text.
fn pg_text(pid: &str, backend_start: Option<&str>, query_start: Option<&str>, query: &str, cut: &str) -> Rows {
    pg_text_in("idle", pid, backend_start, query_start, query, cut)
}

fn pg_text_in(
    state: &str,
    pid: &str,
    backend_start: Option<&str>,
    query_start: Option<&str>,
    query: &str,
    cut: &str,
) -> Rows {
    vec![vec![
        Some(pid.into()),
        backend_start.map(Into::into),
        Some(state.into()),
        query_start.map(Into::into),
        Some(query.into()),
        Some(cut.into()),
    ]]
}

fn pg_ask() -> TextAsk {
    TextAsk {
        session: SessionKey { id: 4211, started: Some(Timestamp(1_000)) },
        query_started: Some(Timestamp(5_000_123)),
        start: "SELECT a".into(),
        seen: Timestamp(6_000_000),
    }
}

#[test]
fn a_pg_text_is_given_only_for_the_same_session_and_statement() {
    let ask = pg_ask();
    let read = |rows: &Rows| decode::pg::query_text(&ask, rows).unwrap();
    let same = pg_text("4211", Some("1000"), Some("5000123"), "SELECT a, b FROM t", "f");
    assert_eq!(
        read(&same),
        FullText::Text {
            text: QueryText { text: "SELECT a, b FROM t".into(), shortened: false, maybe_cut_by_server: false },
            proof: Proof::Same,
        }
    );
    let cut = pg_text("4211", Some("1000"), Some("5000123"), "SELECT a, b", "t");
    assert!(matches!(read(&cut), FullText::Text { text: QueryText { maybe_cut_by_server: true, .. }, .. }));
    // No session with that pid now; another session with the pid (started later).
    assert_eq!(read(&Vec::new()), FullText::Gone);
    assert_eq!(read(&pg_text("4211", Some("2000"), Some("5000123"), "SELECT a", "f")), FullText::Gone);
    // An idle session's last statement is still the one listed while it started then.
    let active = pg_text_in("active", "4211", Some("1000"), Some("5000123"), "SELECT a, b FROM t", "f");
    assert!(matches!(read(&active), FullText::Text { proof: Proof::Same, .. }));
    // Another statement now: a microsecond later, even with the same text.
    assert_eq!(read(&pg_text("4211", Some("1000"), Some("5000124"), "SELECT a, b FROM t", "f")), FullText::Moved);
    // Its activity not tracked now (`track_activities` off): neither start nor text is told.
    let untracked = pg_text_in("disabled", "4211", Some("1000"), None, "", "f");
    assert_eq!(read(&untracked), FullText::Unknown(None));
    let untracked = pg_text_in("disabled", "4211", Some("1000"), Some("5000123"), "SELECT a, b", "f");
    assert_eq!(read(&untracked), FullText::Unknown(None));
    // The same start but another beginning cannot be that statement.
    assert_eq!(read(&pg_text("4211", Some("1000"), Some("5000123"), "UPDATE t", "f")), FullText::Moved);
    // The account may not read it now.
    let hidden = pg_text_in("", "4211", None, None, decode::pg::HIDDEN_QUERY, "f");
    let hidden = vec![vec![hidden[0][0].clone(), None, None, None, hidden[0][4].clone(), hidden[0][5].clone()]];
    assert_eq!(read(&hidden), FullText::Unknown(Some(Need::Grant("pg_read_all_stats"))));
}

#[test]
fn a_pg_text_is_not_given_when_the_list_did_not_tell_the_statement_apart() {
    let row = pg_text("4211", Some("1000"), Some("5000123"), "SELECT a", "f");
    let no_start = TextAsk { query_started: None, ..pg_ask() };
    assert_eq!(decode::pg::query_text(&no_start, &row).unwrap(), FullText::Unknown(None));
    let no_session_start = TextAsk { session: SessionKey { id: 4211, started: None }, ..pg_ask() };
    assert_eq!(decode::pg::query_text(&no_session_start, &row).unwrap(), FullText::Unknown(None));
    // A row of another pid, or two rows, is not what the statement gives.
    assert!(decode::pg::query_text(&pg_ask(), &pg_text("7", Some("1000"), Some("5000123"), "SELECT a", "f")).is_err());
    let two = [row.clone(), row].concat();
    assert!(decode::pg::query_text(&pg_ask(), &two).is_err());
}

/// A row read for a MySQL statement's whole text: the clock at `now` seconds.
fn my_text(now: i64, command: &str, time: &str, info: Option<&str>) -> Rows {
    vec![vec![
        Some((now * 1_000_000).to_string()),
        Some("31".into()),
        Some(command.into()),
        Some(time.into()),
        info.map(Into::into),
        Some("0".into()),
    ]]
}

/// The list read the session at 100.4 s with TIME 10: started 90.4 s by its reckoning.
fn my_ask() -> TextAsk {
    TextAsk {
        session: SessionKey { id: 31, started: None },
        query_started: Some(Timestamp(90_400_000)),
        start: "SELECT SLEEP".into(),
        seen: Timestamp(100_400_000),
    }
}

#[test]
fn a_mysql_text_is_given_while_the_session_runs_a_statement_started_then() {
    let ask = my_ask();
    let read = |rows: &Rows| decode::mysql::query_text(&ask, rows).unwrap();
    let text = |t: &str| FullText::Text {
        text: QueryText { text: t.into(), shortened: false, maybe_cut_by_server: false },
        proof: Proof::Likely,
    };
    // Starts of 91.0, 90.0 and 92.0 s: whole seconds either way of the same start.
    assert_eq!(read(&my_text(103, "Query", "12", Some("SELECT SLEEP(60), 'x'"))), text("SELECT SLEEP(60), 'x'"));
    assert_eq!(read(&my_text(103, "Execute", "13", Some("SELECT SLEEP(60)"))), text("SELECT SLEEP(60)"));
    assert_eq!(read(&my_text(103, "Query", "11", Some("SELECT SLEEP(60)"))), text("SELECT SLEEP(60)"));
    let mut cut = my_text(103, "Query", "12", Some("SELECT SLEEP(60)"));
    cut[0][5] = Some("1".into());
    assert!(matches!(read(&cut), FullText::Text { text: QueryText { maybe_cut_by_server: true, .. }, .. }));
    // A run started 3.6 s later; the session idle; another text; gone.
    assert_eq!(read(&my_text(103, "Query", "9", Some("SELECT SLEEP(60)"))), FullText::Moved);
    assert_eq!(read(&my_text(103, "Sleep", "13", None)), FullText::Moved);
    assert_eq!(read(&my_text(103, "Query", "13", Some("SELECT 1"))), FullText::Moved);
    assert_eq!(read(&Vec::new()), FullText::Gone);
    // 2.6 s apart: the start cannot say which.
    assert_eq!(read(&my_text(103, "Query", "10", Some("SELECT SLEEP(60)"))), FullText::Unknown(None));
}

#[test]
fn a_mysql_text_of_a_statement_listed_young_or_without_a_start_is_not_given() {
    let row = my_text(103, "Query", "13", Some("SELECT SLEEP(60)"));
    // Listed when it had run 4.9 s: a later run of the same text may start within the second.
    let young = TextAsk { seen: Timestamp(95_300_000), ..my_ask() };
    assert_eq!(decode::mysql::query_text(&young, &row).unwrap(), FullText::NotYet);
    let no_start = TextAsk { query_started: None, ..my_ask() };
    assert_eq!(decode::mysql::query_text(&no_start, &row).unwrap(), FullText::Unknown(None));
}

#[test]
fn an_ask_names_the_same_statement_to_the_precision_its_server_tells() {
    let a = pg_ask();
    let later = |us: i64| TextAsk { query_started: Some(Timestamp(us)), seen: Timestamp(9_000_000), ..pg_ask() };
    assert!(a.same_statement(&later(5_000_123), false));
    assert!(!a.same_statement(&later(5_000_124), false));
    assert!(a.same_statement(&later(6_900_000), true));
    assert!(!a.same_statement(&later(7_000_123), true));
    let other = TextAsk { start: "SELECT b".into(), ..pg_ask() };
    assert!(!a.same_statement(&other, false));
    let session = TextAsk { session: SessionKey { id: 7, started: a.session.started }, ..pg_ask() };
    assert!(!a.same_statement(&session, true));
}
mod action;

// Locks.

use super::locks::{self as lk, Blame, Holder, LockList, Tree, Waits};
use std::collections::HashSet;

/// The server's matrix of granted requests for object metadata locks, row by row (`+` granted
/// beside, `-` waits), as `sql/mdl.cc` writes it.
const MDL_OBJECT: &[(&str, &str)] = &[
    ("SHARED", "+ + + + + + + + + -"),
    ("SHARED_HIGH_PRIO", "+ + + + + + + + + -"),
    ("SHARED_READ", "+ + + + + + + + - -"),
    ("SHARED_WRITE", "+ + + + + + - - - -"),
    ("SHARED_WRITE_LOW_PRIO", "+ + + + + + - - - -"),
    ("SHARED_UPGRADABLE", "+ + + + + - + - - -"),
    ("SHARED_READ_ONLY", "+ + + - - + + + - -"),
    ("SHARED_NO_WRITE", "+ + + - - - + - - -"),
    ("SHARED_NO_READ_WRITE", "+ + - - - - - - - -"),
    ("EXCLUSIVE", "- - - - - - - - - -"),
];

#[test]
fn metadata_locks_conflict_as_the_servers_lock_manager_says() {
    let granted: Vec<&str> = MDL_OBJECT.iter().map(|(t, _)| *t).collect();
    for (requested, row) in MDL_OBJECT {
        for (g, cell) in granted.iter().zip(row.split(' ')) {
            assert_eq!(lk::mdl_conflicts("TABLE", requested, g), Some(cell == "-"), "{requested} after {g}");
        }
    }
    // Scoped locks: intention exclusive beside itself, shared beside itself, exclusive alone.
    let scoped = ["INTENTION_EXCLUSIVE", "SHARED", "EXCLUSIVE"];
    let waits = [[false, true, true], [true, false, true], [true, true, true]];
    for (r, row) in scoped.iter().zip(waits) {
        for (g, w) in scoped.iter().zip(row) {
            for kind in ["GLOBAL", "SCHEMA", "BACKUP LOCK", "BACKUP_LOCK", "COMMIT"] {
                assert_eq!(lk::mdl_conflicts(kind, r, g), Some(w), "{kind} {r} after {g}");
            }
        }
    }
    // What the table does not know is not guessed.
    assert_eq!(lk::mdl_conflicts("TABLE", "INTENTION_EXCLUSIVE", "SHARED"), None);
    assert_eq!(lk::mdl_conflicts("GLOBAL", "SHARED_READ", "SHARED"), None);
    assert_eq!(lk::mdl_conflicts("SOMETHING NEW", "EXCLUSIVE", "SHARED"), None);
    assert_eq!(lk::mdl_conflicts("TABLE", "EXCLUSIVE", "SOMETHING_NEW"), None);
}

/// The lock chain recorded on PostgreSQL 14 (`pg14_locks`): 13617 holds a row of `zz_mon_t` in
/// an open transaction; 13896 updates the same row; 14091 alters the table (behind both); 14315
/// reads it (behind the ALTER); prepared transaction `zz_mon_gid` holds `zz_mon_p`, into which
/// 14485 inserts; idle 14652 holds advisory lock 7, which 14819 asks for.
fn pg14_locks() -> (SessionList, LockList) {
    let (sample, _) = decode::pg::counters(&single(fixture!("pg14_locks/counters")), Own::default()).unwrap();
    let (list, _) = decode::pg::sessions(&single(fixture!("pg14_locks/sessions_without_idle")), NO_IDLE).unwrap();
    let locks = decode::pg::locks(&single(fixture!("pg14_locks/locks")), sample.time).unwrap();
    (list, locks)
}

#[test]
fn postgres_lock_waits_read_with_what_is_held_on_them() {
    let (_, locks) = pg14_locks();
    assert!(!locks.blocks, "the session list names PostgreSQL's blockers");
    assert_eq!((locks.waits.len(), locks.more), (5, 0));
    let alter = locks.waits_of(14091).next().unwrap();
    assert_eq!(alter.mode, "AccessExclusiveLock");
    assert_eq!((alter.object.kind.as_str(), alter.object.name.as_deref()), ("relation", Some("zz_mon_t")));
    assert!(alter.since.is_some(), "waitstart from 14 on");
    let held: Vec<(Holder, Vec<String>)> = alter.holders.iter().map(|h| (h.holder.clone(), h.modes.clone())).collect();
    assert_eq!(
        held,
        [
            (Holder::Session(13617), vec!["RowExclusiveLock".to_string()]),
            (Holder::Session(13896), vec!["RowExclusiveLock".to_string()]),
        ]
    );
    assert!(alter.holders.iter().all(|h| h.blame == Blame::Holds && !h.waiting));
    let row = locks.waits_of(13896).next().unwrap();
    assert_eq!(
        (row.object.kind.as_str(), row.object.detail.as_deref(), row.mode.as_str()),
        ("transactionid", Some("736"), "ShareLock")
    );
    // The prepared transaction is named by its transaction id lock.
    let insert = locks.waits_of(14485).next().unwrap();
    match &insert.holders[..] {
        [h] => match &h.holder {
            Holder::Prepared(Some(p)) => {
                assert_eq!(
                    (p.gid.as_str(), p.owner.as_deref(), p.database.as_deref()),
                    ("zz_mon_gid", Some("datarig"), Some("datarig"))
                );
                assert!(p.since.is_some());
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
    // A waiter's own lock on the object (none here) is never one it waits for.
    assert!(locks.waits.iter().all(|w| w.holders.iter().all(|h| h.holder != Holder::Session(w.waiter.id))));
}

#[test]
fn the_tree_puts_root_blockers_first_and_each_session_once() {
    let (list, locks) = pg14_locks();
    let waits = lk::edges(Some(&list), Some(&locks), true);
    assert!(waits.alone.is_empty());
    let tree = Tree::build(&waits);
    let lines: Vec<(String, usize)> = tree
        .lines
        .iter()
        .map(|l| {
            let name = match &l.node {
                Holder::Session(id) => id.to_string(),
                Holder::Prepared(Some(p)) => p.gid.clone(),
                other => format!("{other:?}"),
            };
            (name, l.depth)
        })
        .collect();
    let want: Vec<(String, usize)> = [
        ("13617", 0),
        ("14091", 1),
        ("14315", 2),
        ("13896", 1),
        ("14652", 0),
        ("14819", 1),
        ("zz_mon_gid", 0),
        ("14485", 1),
    ]
    .iter()
    .map(|(n, d)| (n.to_string(), *d))
    .collect();
    assert_eq!(lines, want);
    assert_eq!(tree.waiting, 5);
    assert_eq!(tree.lines[0].behind, 3);
    // The ALTER also waits for the UPDATE before it.
    assert_eq!(tree.lines[1].also, [(Holder::Session(13896), Blame::Proven)]);
    // From the reader behind the ALTER, the way up to the root is the server's word.
    assert_eq!(tree.root_of(2), Some((0, true)));
    // Without the lock read the prepared transaction is still a root, not told apart.
    let bare = Tree::build(&lk::edges(Some(&list), None, true));
    assert!(bare.lines.iter().any(|l| l.node == Holder::Prepared(None) && l.depth == 0));
}

#[test]
fn a_cycle_of_waits_is_shown_once_from_its_smallest_session() {
    let e = |w: i64, h: i64| lk::Edge { waiter: w, holder: Holder::Session(h), blame: Blame::Proven };
    let tree = Tree::build(&Waits { edges: vec![e(30, 10), e(10, 20), e(20, 10)], alone: vec![40] });
    let lines: Vec<(Holder, usize, bool)> = tree.lines.iter().map(|l| (l.node.clone(), l.depth, l.cycle)).collect();
    assert_eq!(
        lines,
        [
            (Holder::Session(10), 0, true),
            (Holder::Session(20), 1, false),
            (Holder::Session(30), 1, false),
            (Holder::Session(40), 0, false),
        ]
    );
    assert!(tree.lines[3].alone);
    assert_eq!(tree.waiting, 4);
    // The root of a cycle waits for a session under it.
    assert_eq!(tree.lines[0].also, [(Holder::Session(20), Blame::Proven)]);
}

fn mysql_root() -> MySqlMonitor {
    decode::mysql::probe(&fixture!("my84_root/probe")).unwrap()
}

fn cells(values: &[Option<&str>]) -> Vec<Option<String>> {
    values.iter().map(|v| v.map(str::to_owned)).collect()
}

/// Rows shaped as MySQL 8.4 gives them: 7 waits for a row 5 holds; 8 waits for a row the XA
/// transaction 4242 (no session) holds; a metadata lock request of 9 (`EXCLUSIVE` for `ALTER`)
/// on `shop.orders`, where 5 and 6 hold `SHARED_READ`, 9 itself `SHARED_UPGRADABLE`, and 11 a
/// lock on another table; 10 asks for `SHARED_READ` behind the `ALTER`, beside every granted
/// lock (it waits for the pending request, which is not modelled).
fn mysql_lock_answers() -> Vec<Answer> {
    let s = Some;
    let n = None;
    let rows = vec![cells(&[s("7"), s("5"), s("1001")]), cells(&[s("8"), n, s("4242")])];
    let mdl = vec![
        cells(&[s("9"), s("109"), s("TABLE"), s("shop"), s("orders"), s("EXCLUSIVE"), s("TRANSACTION"), s("PENDING")]),
        cells(&[
            s("10"),
            s("110"),
            s("TABLE"),
            s("shop"),
            s("orders"),
            s("SHARED_READ"),
            s("TRANSACTION"),
            s("PENDING"),
        ]),
        cells(&[
            s("5"),
            s("105"),
            s("TABLE"),
            s("shop"),
            s("orders"),
            s("SHARED_READ"),
            s("TRANSACTION"),
            s("GRANTED"),
        ]),
        cells(&[
            s("6"),
            s("106"),
            s("TABLE"),
            s("shop"),
            s("orders"),
            s("SHARED_READ"),
            s("TRANSACTION"),
            s("GRANTED"),
        ]),
        cells(&[
            s("9"),
            s("109"),
            s("TABLE"),
            s("shop"),
            s("orders"),
            s("SHARED_UPGRADABLE"),
            s("TRANSACTION"),
            s("GRANTED"),
        ]),
        cells(&[
            s("11"),
            s("111"),
            s("TABLE"),
            s("shop"),
            s("items"),
            s("SHARED_WRITE"),
            s("TRANSACTION"),
            s("GRANTED"),
        ]),
    ];
    vec![Answer::Rows(rows), Answer::Rows(mdl)]
}

#[test]
fn mysql_row_lock_waits_name_their_blockers_and_metadata_locks_their_incompatible_holders() {
    let my = mysql_root();
    let (locks, hidden) = decode::mysql::locks(&mysql_lock_answers(), &my, Timestamp(1)).unwrap();
    assert!(hidden.is_empty() && locks.blocks);
    let row = locks.waits_of(7).next().unwrap();
    // What it is on is not read (the engine would go through every lock it holds).
    assert_eq!((row.object.kind.as_str(), row.object.name.as_deref(), row.mode.as_str()), ("RECORD", None, ""));
    assert_eq!((row.holders[0].holder.clone(), row.holders[0].blame), (Holder::Session(5), Blame::Proven));
    let xa = locks.waits_of(8).next().unwrap();
    assert_eq!(xa.holders[0].holder, Holder::Transaction("4242".into()));
    let alter = locks.waits_of(9).next().unwrap();
    assert_eq!((alter.object.kind.as_str(), alter.mode.as_str()), ("TABLE", "EXCLUSIVE"));
    let holders: Vec<(Holder, Blame)> = alter.holders.iter().map(|h| (h.holder.clone(), h.blame)).collect();
    // Its own lock and the other table's are left out.
    assert_eq!(holders, [(Holder::Session(5), Blame::Incompatible), (Holder::Session(6), Blame::Incompatible)]);
    // Beside every granted lock: it waits behind a request, which is not modelled.
    assert!(locks.waits_of(10).next().unwrap().holders.is_empty());
    let waits = lk::edges(None, Some(&locks), false);
    assert_eq!(waits.alone, [10]);
    let tree = Tree::build(&waits);
    let at = tree.lines.iter().position(|l| l.node == Holder::Session(9)).unwrap();
    // Two holders may block the ALTER: no root is named for it.
    assert_eq!(tree.root_of(at).map(|(_, sure)| sure), Some(false));
    let one = |locks: &LockList| {
        let mut l = locks.clone();
        for w in &mut l.waits {
            w.holders.retain(|h| h.holder != Holder::Session(6));
        }
        l
    };
    let tree = Tree::build(&lk::edges(None, Some(&one(&locks)), false));
    let at = tree.lines.iter().position(|l| l.node == Holder::Session(9)).unwrap();
    assert_eq!(tree.root_of(at).map(|(_, sure)| sure), Some(true), "the only holder of an incompatible lock");
}

#[test]
fn mysql_lock_reads_refused_or_off_say_what_would_show_them() {
    let my = mysql_root();
    let refused = Answer::Error { code: Some(1142), message: "SELECT command denied".into() };
    let (locks, hidden) = decode::mysql::locks(
        &[refused],
        &MySqlMonitor { metadata_locks: Readable::Refused, ..my.clone() },
        Timestamp(1),
    )
    .unwrap();
    assert!(locks.waits.is_empty());
    assert_eq!(
        hidden,
        [
            Hidden { part: Part::RowLockWaits, need: Need::Grant("SELECT on performance_schema") },
            Hidden { part: Part::MetadataLockWaits, need: Need::Grant("SELECT on performance_schema") },
        ]
    );
    let (_, hidden) = decode::mysql::locks(
        &[Answer::Rows(Vec::new())],
        &MySqlMonitor { metadata_locks: Readable::Unknown, ..my.clone() },
        Timestamp(1),
    )
    .unwrap();
    assert_eq!(hidden, [Hidden { part: Part::MetadataLockWaits, need: Need::Setting("wait/lock/metadata/sql/mdl") }]);
    // Another failure is not an empty list.
    let failed = Answer::Error { code: Some(3024), message: "maximum statement execution time exceeded".into() };
    assert!(decode::mysql::locks(&[failed, Answer::Rows(Vec::new())], &my, Timestamp(1)).is_err());
}

#[test]
fn a_waits_lock_read_older_than_the_list_drops_waits_that_are_over() {
    let my = mysql_root();
    let (locks, _) = decode::mysql::locks(&mysql_lock_answers(), &my, Timestamp(1)).unwrap();
    let mut list = decode::mysql::sessions(&rows_of_sessions(), &my, IDLE).unwrap().0;
    let waiting = |id: i64, waits: bool| SessionRow {
        key: SessionKey { id, started: None },
        wait: waits.then(|| Wait { class: WaitClass::Lock, name: "row lock".into() }),
        ..list.rows[0].clone()
    };
    let mut alter = waiting(9, true);
    alter.wait = Some(Wait { class: WaitClass::Lock, name: "table metadata lock".into() });
    list.rows = vec![waiting(7, false), waiting(8, true), alter];
    let waits = lk::edges(Some(&list), Some(&locks), true);
    let waiters: HashSet<i64> = waits.edges.iter().map(|e| e.waiter).collect();
    // 7 no longer waits; 10 is not listed though every session is: both are over.
    assert_eq!(waiters, HashSet::from([8, 9]));
    assert!(waits.alone.is_empty());
    // A list of only some sessions keeps a wait of one it does not show.
    let waits = lk::edges(Some(&list), Some(&locks), false);
    assert_eq!(waits.alone, [10]);
}

#[test]
fn the_lock_tables_are_read_only_when_a_session_may_wait() {
    let my = mysql_root();
    let sessions = rows_of_sessions().remove(0);
    let quiet: Rows = sessions
        .iter()
        .map(|r| {
            let mut r = r.clone();
            r[7] = None;
            r[12] = None;
            r
        })
        .collect();
    assert!(!decode::mysql::may_wait(&quiet, &my).unwrap());
    let mut row_wait = quiet.clone();
    row_wait[0][12] = Some("LOCK WAIT".into());
    assert!(decode::mysql::may_wait(&row_wait, &my).unwrap());
    let mut mdl_wait = quiet.clone();
    mdl_wait[0][7] = Some("Waiting for table metadata lock".into());
    assert!(decode::mysql::may_wait(&mdl_wait, &my).unwrap());
    // Without PROCESS other accounts' waits are not seen: read.
    assert!(decode::mysql::may_wait(&quiet, &MySqlMonitor { process: Readable::Unknown, ..my }).unwrap());
}

#[test]
fn postgres_lock_modes_conflict_as_the_servers_table_says() {
    assert_eq!(lk::pg_conflicts("ShareLock", "ExclusiveLock"), Some(true));
    assert_eq!(lk::pg_conflicts("ShareLock", "AccessShareLock"), Some(false));
    assert_eq!(lk::pg_conflicts("ShareLock", "RowExclusiveLock"), Some(true));
    assert_eq!(lk::pg_conflicts("ShareLock", "ShareLock"), Some(false));
    assert_eq!(lk::pg_conflicts("AccessShareLock", "AccessExclusiveLock"), Some(true));
    assert_eq!(lk::pg_conflicts("RowExclusiveLock", "RowExclusiveLock"), Some(false));
    assert_eq!(lk::pg_conflicts("ShareUpdateExclusiveLock", "ShareUpdateExclusiveLock"), Some(true));
    assert_eq!(lk::pg_conflicts("SIReadLock", "AccessShareLock"), None);
}

/// A session waiting for a prepared transaction, with the lock read's wait of `prepared`
/// holders (gid, mode) on its object for the statement that started at `statement`.
fn prepared_wait(prepared: &[(&str, &str)], statement: Timestamp) -> (SessionList, LockList) {
    let (list, mut locks) = pg14_locks();
    let w = locks.waits.iter_mut().find(|w| w.waiter.id == 14485).unwrap();
    w.query_started = Some(statement);
    w.mode = "ShareLock".into();
    w.holders = prepared
        .iter()
        .map(|(gid, mode)| lk::LockHolder {
            holder: Holder::Prepared(Some(lk::Prepared {
                gid: (*gid).into(),
                owner: None,
                database: None,
                since: None,
            })),
            modes: vec![(*mode).into()],
            waiting: false,
            blame: Blame::Holds,
        })
        .collect();
    (list, locks)
}

fn prepared_blockers(list: &SessionList, locks: &LockList) -> Vec<Holder> {
    lk::edges(Some(list), Some(locks), true).edges.into_iter().filter(|e| e.waiter == 14485).map(|e| e.holder).collect()
}

#[test]
fn only_a_prepared_transaction_whose_lock_conflicts_is_named_as_the_blocker() {
    let (list, _) = pg14_locks();
    let started = list.rows.iter().find(|r| r.key.id == 14485).unwrap().query_started.unwrap();
    let (list, locks) = prepared_wait(&[("reads", "AccessShareLock"), ("writes", "RowExclusiveLock")], started);
    let named: Vec<String> = prepared_blockers(&list, &locks)
        .into_iter()
        .map(|h| match h {
            Holder::Prepared(Some(p)) => p.gid,
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(named, ["writes"], "a share lock waits for the row exclusive lock only");
}

#[test]
fn a_lock_read_of_an_earlier_statement_does_not_name_the_prepared_blocker() {
    let (list, _) = pg14_locks();
    let started = list.rows.iter().find(|r| r.key.id == 14485).unwrap().query_started.unwrap();
    // The lock read saw the session's previous statement wait for another prepared transaction.
    let (list, locks) = prepared_wait(&[("gone", "RowExclusiveLock")], Timestamp(started.0 - 5_000_000));
    assert_eq!(prepared_blockers(&list, &locks), [Holder::Prepared(None)]);
}

#[test]
fn a_session_waiting_for_a_user_level_lock_waits_for_a_lock() {
    let my = mysql_root();
    let mut rows = rows_of_sessions().remove(0);
    for r in rows.iter_mut() {
        r[7] = None;
        r[12] = None;
    }
    assert!(!decode::mysql::may_wait(&rows, &my).unwrap());
    // `GET_LOCK()` waiting: the server's state for it is `User lock`.
    rows[0][7] = Some("User lock".into());
    assert!(decode::mysql::may_wait(&rows, &my).unwrap());
    let (list, _) = decode::mysql::sessions(&[rows], &my, IDLE).unwrap();
    assert_eq!(list.rows[0].wait, Some(Wait { class: WaitClass::Lock, name: "user lock".into() }));
}

#[test]
fn a_prepared_transaction_holding_only_a_lock_of_a_mode_not_known_is_not_named() {
    let (list, _) = pg14_locks();
    let started = list.rows.iter().find(|r| r.key.id == 14485).unwrap().query_started.unwrap();
    let (list, locks) = prepared_wait(&[("serializable", "SIReadLock"), ("writes", "RowExclusiveLock")], started);
    let named: Vec<String> = prepared_blockers(&list, &locks)
        .into_iter()
        .map(|h| match h {
            Holder::Prepared(Some(p)) => p.gid,
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(named, ["writes"]);
}

#[test]
fn a_mysql_wait_of_a_later_statement_is_not_told_by_an_older_lock_read() {
    let my = mysql_root();
    let (mut locks, _) = decode::mysql::locks(&mysql_lock_answers(), &my, Timestamp(1)).unwrap();
    let row = locks.waits.iter_mut().find(|w| w.waiter.id == 7).unwrap();
    row.query_started = Some(Timestamp(100_000_000));
    let mut list = decode::mysql::sessions(&rows_of_sessions(), &my, IDLE).unwrap().0;
    let base = list.rows[0].clone();
    let session = |wait: &str, started: i64| SessionRow {
        key: SessionKey { id: 7, started: None },
        wait: Some(Wait { class: WaitClass::Lock, name: wait.into() }),
        query_started: Some(Timestamp(started)),
        ..base.clone()
    };
    let blockers = |list: &SessionList| -> Vec<Holder> {
        lk::edges(Some(list), Some(&locks), false)
            .edges
            .into_iter()
            .filter(|e| e.waiter == 7)
            .map(|e| e.holder)
            .collect()
    };
    // The same statement (its start a second off, as MySQL tells it), the same row lock wait.
    list.rows = vec![session("row lock", 101_000_000)];
    assert_eq!(blockers(&list), [Holder::Session(5)]);
    // A new statement waiting for a metadata lock now: the row lock's blocker is not its.
    list.rows = vec![session("table metadata lock", 130_000_000)];
    assert!(blockers(&list).is_empty());
    assert!(lk::edges(Some(&list), Some(&locks), false).alone.contains(&7), "it waits all the same");
}

#[test]
fn a_session_waiting_when_the_lock_tables_were_refused_is_still_shown_waiting() {
    let my = mysql_root();
    let refused = Answer::Error { code: Some(1142), message: "denied".into() };
    let (locks, _) = decode::mysql::locks(&[refused, Answer::Rows(Vec::new())], &my, Timestamp(1)).unwrap();
    let mut list = decode::mysql::sessions(&rows_of_sessions(), &my, IDLE).unwrap().0;
    list.rows.truncate(1);
    list.rows[0].wait = Some(Wait { class: WaitClass::Lock, name: "row lock".into() });
    let id = list.rows[0].key.id;
    let tree = Tree::build(&lk::edges(Some(&list), Some(&locks), true));
    assert_eq!(tree.waiting, 1);
    assert!(tree.lines.iter().any(|l| l.node == Holder::Session(id) && l.alone));
}

#[test]
fn a_row_wait_the_list_cannot_see_keeps_its_blocker() {
    // With SELECT on performance_schema but not PROCESS: no INNODB_TRX, so the list does not
    // show the row lock wait (and cannot tell it is over).
    let my = MySqlMonitor { process: Readable::Unknown, innodb_trx: Readable::Refused, ..mysql_root() };
    let (locks, _) = decode::mysql::locks(&mysql_lock_answers(), &my, Timestamp(1)).unwrap();
    let mut list = decode::mysql::sessions(&rows_of_sessions(), &my, IDLE).unwrap().0;
    list.rows.truncate(1);
    list.rows[0].key.id = 7;
    list.rows[0].wait = None;
    list.rows[0].tx = Tx::Unknown;
    let waits = lk::edges(Some(&list), Some(&locks), false);
    assert!(waits.edges.iter().any(|e| e.waiter == 7 && e.holder == Holder::Session(5)), "{waits:?}");
    // Known to hold no transaction: the wait is over.
    list.rows[0].tx = Tx::None;
    let waits = lk::edges(Some(&list), Some(&locks), false);
    assert!(!waits.edges.iter().any(|e| e.waiter == 7), "{waits:?}");
}

#[test]
fn a_blocker_that_waits_itself_for_a_holder_not_found_is_marked_waiting() {
    let e = |w: i64, h: i64| lk::Edge { waiter: w, holder: Holder::Session(h), blame: Blame::Proven };
    // B (20) waits for A (10); A waits too (GET_LOCK), its holder not read.
    let tree = Tree::build(&Waits { edges: vec![e(20, 10)], alone: vec![10] });
    assert_eq!(tree.lines.len(), 2);
    assert!(tree.lines[0].alone && tree.lines[0].node == Holder::Session(10));
    assert_eq!(tree.waiting, 2);
    assert_eq!(tree.root_of(1), Some((0, false)), "A is not the root of the waits");
}
