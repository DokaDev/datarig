use super::{IDLE, by_application, by_id, load, pg, rows, single};
use crate::driver::{DbCommand, DbError, DbEvent};
use crate::monitor::action::*;
use crate::monitor::decode;
use crate::monitor::format::Route;
use crate::monitor::{
    Flavour, MonitorFormat, MySqlMonitor, Need, QueryText, SessionKey, SessionKind, SessionList, SessionRow,
    SessionState, Timestamp, Visibility,
};

macro_rules! fixture {
    ($name:literal) => {
        load(include_str!(concat!("../fixtures/", $name, ".json")))
    };
}

fn pg_format(route: Route) -> MonitorFormat {
    MonitorFormat::Postgres(crate::monitor::PgMonitor { route, ..pg(170011) })
}

fn pg_sessions() -> SessionList {
    decode::pg::sessions(&single(fixture!("pg17_locks/sessions")), IDLE).unwrap().0
}

fn my_root() -> (MonitorFormat, SessionList) {
    let m = decode::mysql::probe(&fixture!("my84_root/probe")).unwrap();
    let (list, _) = decode::mysql::sessions(&rows(fixture!("my84_root/sessions")), &m, IDLE).unwrap();
    (MonitorFormat::MySql(m), list)
}

fn my_unprivileged() -> (MonitorFormat, SessionList) {
    let m = decode::mysql::probe(&fixture!("my84_unprivileged/probe")).unwrap();
    let (list, _) = decode::mysql::sessions(&rows(fixture!("my84_unprivileged/sessions")), &m, IDLE).unwrap();
    (MonitorFormat::MySql(m), list)
}

fn offer(warnings: &[Warning]) -> Decision {
    Decision::Offer(Ask::Modal { warnings: warnings.to_vec() })
}

const LINE: Decision = Decision::Offer(Ask::Line);
const BOTH: [ActionKind; 2] = [ActionKind::Cancel, ActionKind::Terminate];

#[test]
fn a_read_only_profile_refuses_both_actions() {
    let pg = pg_format(Route::Direct);
    let list = pg_sessions();
    let (my, my_list) = my_root();
    for kind in BOTH {
        let row = by_application(&list, "zz_mon_b");
        assert_eq!(decide(&pg, row, true, kind), Decision::Refuse(Refusal::ReadOnly), "{kind:?}");
        assert_eq!(decide(&my, by_id(&my_list, 63114), true, kind), Decision::Refuse(Refusal::ReadOnly));
    }
}

#[test]
fn the_monitors_own_connection_has_no_actions() {
    let pg = pg_format(Route::Direct);
    let list = pg_sessions();
    let own = list.rows.iter().find(|r| r.monitor).unwrap();
    let (my, my_list) = my_root();
    let my_own = my_list.rows.iter().find(|r| r.monitor).unwrap();
    for kind in BOTH {
        assert_eq!(decide(&pg, own, false, kind), Decision::Refuse(Refusal::MonitorConnection));
        assert_eq!(decide(&my, my_own, false, kind), Decision::Refuse(Refusal::MonitorConnection));
    }
}

#[test]
fn a_hidden_postgres_session_needs_pg_read_all_stats() {
    let pg = pg_format(Route::Direct);
    let (list, _) = decode::pg::sessions(&single(fixture!("pg17_unprivileged/sessions")), IDLE).unwrap();
    let hidden = by_application(&list, "zz_mon_b");
    assert_eq!(hidden.visible, Visibility::Hidden);
    assert_eq!(target(&pg, hidden), None);
    for kind in BOTH {
        assert_eq!(
            decide(&pg, hidden, false, kind),
            Decision::Refuse(Refusal::Hidden(Need::Grant("pg_read_all_stats")))
        );
    }
}

#[test]
fn background_processes_replication_and_parallel_workers_are_refused() {
    let pg = pg_format(Route::Direct);
    let list = pg_sessions();
    let checkpointer = by_id(&list, 26);
    assert!(matches!(checkpointer.kind, SessionKind::Background(_)));
    let active = by_application(&list, "zz_mon_b").clone();
    let replication = SessionRow { kind: SessionKind::Replication, ..active.clone() };
    let worker = SessionRow { kind: SessionKind::ParallelWorker { leader: Some(1) }, ..active };
    let (my, my_list) = my_root();
    for kind in BOTH {
        for row in [checkpointer, &replication, &worker] {
            assert_eq!(decide(&pg, row, false, kind), Decision::Refuse(Refusal::Background), "{kind:?} {:?}", row.kind);
        }
        assert_eq!(decide(&my, by_id(&my_list, 5), false, kind), Decision::Refuse(Refusal::Background));
    }
}

#[test]
fn postgres_asks_a_line_for_a_cancel_and_a_dialog_for_an_end() {
    let pg = pg_format(Route::Direct);
    let list = pg_sessions();
    let active = by_application(&list, "zz_mon_b");
    let idle = by_application(&list, "zz_mon_a");
    assert_eq!(idle.state, SessionState::IdleInTx);
    assert_eq!(decide(&pg, active, false, ActionKind::Cancel), LINE);
    assert_eq!(decide(&pg, idle, false, ActionKind::Cancel), Decision::Refuse(Refusal::NoStatement));
    assert_eq!(decide(&pg, active, false, ActionKind::Terminate), offer(&[]));
    assert_eq!(decide(&pg, idle, false, ActionKind::Terminate), offer(&[]), "direct: an idle session may be ended");
    let never =
        SessionRow { query_started: None, statement: Some(StatementStamp::Pg { query_start: None }), ..active.clone() };
    assert_eq!(decide(&pg, &never, false, ActionKind::Cancel), Decision::Refuse(Refusal::NoStatement));
}

#[test]
fn behind_a_pooler_an_idle_backend_is_not_ended_and_an_end_warns() {
    let list = pg_sessions();
    let active = by_application(&list, "zz_mon_b");
    let idle = by_application(&list, "zz_mon_a");
    // Not verified direct is treated as pooled.
    for route in [Route::Pooled, Route::Unknown] {
        let pg = pg_format(route);
        assert_eq!(decide(&pg, idle, false, ActionKind::Terminate), Decision::Refuse(Refusal::PooledIdle));
        assert_eq!(decide(&pg, active, false, ActionKind::Terminate), offer(&[Warning::PoolerSharedBackend]));
        assert_eq!(decide(&pg, active, false, ActionKind::Cancel), LINE);
    }
}

fn my_row(list: &SessionList, id: i64, text: &str, shortened: bool) -> SessionRow {
    let row = by_id(list, id).clone();
    SessionRow { query: Some(QueryText { text: text.into(), shortened, maybe_cut_by_server: false }), ..row }
}

#[test]
fn mysql_cancel_asks_a_line_only_for_a_plain_read() {
    let (my, list) = my_root();
    // 63114 runs a statement event (a strong stamp).
    let read = my_row(&list, 63114, "SELECT id, v FROM zz_mon_t WHERE id = 1", false);
    assert_eq!(decide(&my, &read, false, ActionKind::Cancel), LINE);
    let half = offer(&[Warning::MySqlMayLeaveHalfDone]);
    for text in [
        "UPDATE zz_mon_t SET v=v+1 WHERE id=1",
        "SELECT shop.recalc(1)",
        "SELECT * FROM zz_mon_t FOR UPDATE",
        "INSERT INTO zz_mon_t SELECT * FROM zz_mon_u",
        "not a statement at all",
        "/*! SELECT 1 */",
        "CALL shop.nightly()",
    ] {
        let row = my_row(&list, 63114, text, false);
        assert_eq!(decide(&my, &row, false, ActionKind::Cancel), half, "{text}");
    }
    // A read the list holds only the start of cannot be judged.
    let cut = by_id(&list, 63119);
    assert!(cut.query.as_ref().unwrap().shortened);
    assert_eq!(decide(&my, cut, false, ActionKind::Cancel), half);
    for text in
        ["ALTER TABLE t ADD COLUMN x INT", "optimize table t", " (CHECK TABLE t)", "LOAD DATA INFILE 'f' INTO TABLE t"]
    {
        let row = my_row(&list, 63114, text, true);
        assert_eq!(decide(&my, &row, false, ActionKind::Cancel), offer(&[Warning::MySqlMaintenance]), "{text}");
    }
    let no_text = SessionRow { query: None, ..by_id(&list, 63114).clone() };
    assert_eq!(decide(&my, &no_text, false, ActionKind::Cancel), half, "unknown is not a read");
    assert_eq!(decide(&my, by_id(&list, 63113), false, ActionKind::Cancel), Decision::Refuse(Refusal::NoStatement));
}

#[test]
fn a_weak_mysql_stamp_is_said_so() {
    let (my, list) = my_root();
    // The ALTER's thread is not instrumented: its statement has no event.
    let alter = by_id(&list, 63118);
    assert!(alter.statement.as_ref().is_some_and(StatementStamp::weak));
    assert_eq!(
        decide(&my, alter, false, ActionKind::Cancel),
        offer(&[Warning::WeakIdentity, Warning::MySqlMaintenance])
    );
    let (my, list) = my_unprivileged();
    let read = my_row(&list, 63114, "SELECT 1", false);
    assert_eq!(decide(&my, &read, false, ActionKind::Cancel), offer(&[Warning::WeakIdentity]));
}

#[test]
fn ending_a_mysql_session_says_non_transactional_changes_stay() {
    let (my, list) = my_root();
    for id in [63113, 63114, 63118] {
        assert_eq!(
            decide(&my, by_id(&list, id), false, ActionKind::Terminate),
            offer(&[Warning::MySqlTerminateKeepsNonTx]),
            "{id}"
        );
    }
}

#[test]
fn targets_carry_the_rows_session_and_statement() {
    let pg = pg_format(Route::Direct);
    let list = pg_sessions();
    let active = by_application(&list, "zz_mon_b");
    let t = target(&pg, active).unwrap();
    assert_eq!(t.key, SessionKey { id: 479561, started: Some(Timestamp(1_794_060_043_772_950)) });
    assert_eq!(t.user.as_deref(), Some("datarig"));
    assert_eq!(
        (t.thread, t.stamp),
        (None, Some(StatementStamp::Pg { query_start: Some(Timestamp(1_794_060_043_777_437)) }))
    );

    let (my, list) = my_root();
    let update = target(&my, by_id(&list, 63114)).unwrap();
    assert_eq!(update.key, SessionKey { id: 63114, started: None });
    assert_eq!(update.thread, Some(63154));
    assert_eq!(update.stamp, Some(StatementStamp::MySqlEvent { thread_id: 63154, event_id: 1207 }));
    let alter = target(&my, by_id(&list, 63118)).unwrap();
    assert_eq!(
        alter.stamp,
        Some(StatementStamp::MySqlWeak {
            command: "Query".into(),
            info_md5: Some("b7b2a88be5a2ced1d67a6f7293da518d".into()),
            time: 4
        })
    );
    // Idle: no statement runs, so there is none to name.
    assert_eq!(target(&my, by_id(&list, 63113)).unwrap().stamp, None);
    assert_eq!(by_id(&list, 5).statement, None);

    let (my, list) = my_unprivileged();
    let weak = target(&my, by_id(&list, 63114)).unwrap();
    assert_eq!(weak.thread, None);
    assert!(matches!(weak.stamp, Some(StatementStamp::MySqlWeak { ref command, time: 5, .. }) if command == "Query"));
    let m = decode::mysql::probe(&fixture!("my80/probe")).unwrap();
    let (list, _) = decode::mysql::sessions(&rows(fixture!("my80/sessions")), &m, IDLE).unwrap();
    assert_eq!(by_id(&list, 18).statement, Some(StatementStamp::MySqlEvent { thread_id: 58, event_id: 412 }));
}

fn my_format(flavour: Flavour, version: u32) -> MonitorFormat {
    let (MonitorFormat::MySql(m), _) = my_root() else { unreachable!() };
    MonitorFormat::MySql(MySqlMonitor { flavour, version, ..m })
}

const FLAVOURS: [Flavour; 6] = [
    Flavour::Standard,
    Flavour::AmazonRds,
    Flavour::AmazonAurora,
    Flavour::GoogleCloudSql,
    Flavour::GoogleAlloyDb,
    Flavour::AzureDatabase,
];

#[test]
fn every_flavour_has_a_route() {
    for flavour in FLAVOURS {
        let pg = MonitorFormat::Postgres(crate::monitor::PgMonitor { flavour, ..pg(170011) });
        let r = route(&pg);
        assert_eq!((r.engine, r.flavour, r.fallback), (Engine::Postgres, flavour, None), "{flavour:?}");
        assert!(r.guidance.grants.contains(&"pg_signal_backend"), "{flavour:?}");
        let r = route(&my_format(flavour, 80411));
        assert_eq!(r.engine, Engine::MySql);
        assert!(!r.guidance.grants.contains(&"SUPER"), "{flavour:?}: never recommends SUPER");
        assert_eq!(r.guidance.grants, ["CONNECTION_ADMIN"]);
        let (listed, fallback) = match flavour {
            Flavour::AmazonRds | Flavour::AmazonAurora => (flavour, Some(("mysql.rds_kill_query", "mysql.rds_kill"))),
            Flavour::AzureDatabase => (flavour, Some(("mysql.az_kill_query", "mysql.az_kill"))),
            // Not listed for MySQL: the standard route.
            Flavour::Standard | Flavour::GoogleCloudSql | Flavour::GoogleAlloyDb => (Flavour::Standard, None),
        };
        assert_eq!(r.flavour, listed, "{flavour:?}");
        assert_eq!(
            r.fallback.map(|f| (f.procedure(ActionKind::Cancel), f.procedure(ActionKind::Terminate))),
            fallback,
            "{flavour:?}"
        );
        assert!(r.fallback.is_none_or(|f| f.on == [MYSQL_KILL_DENIED]));
    }
    let service = |f| route(&MonitorFormat::Postgres(crate::monitor::PgMonitor { flavour: f, ..pg(170011) })).guidance;
    assert_eq!(service(Flavour::AmazonRds).service_users, ["rdsadmin"]);
    assert_eq!(service(Flavour::GoogleCloudSql).grants, ["cloudsqlsuperuser", "pg_signal_backend"]);
    assert_eq!(service(Flavour::AzureDatabase).service_users, ["azuresu"]);
    assert_eq!(service(Flavour::GoogleAlloyDb).grants, ["alloydbsuperuser", "pg_signal_backend"]);
}

fn who(user: &str) -> ActionTarget {
    ActionTarget { key: SessionKey { id: 7, started: None }, user: Some(user.into()), thread: None, stamp: None }
}

#[test]
fn server_refusals_say_what_would_allow_the_action() {
    let pg_as = |signal_backend, superuser, flavour| {
        MonitorFormat::Postgres(crate::monitor::PgMonitor { signal_backend, superuser, flavour, ..pg(170011) })
    };
    let refused = NativeError::Pg { sqlstate: PG_INSUFFICIENT_PRIVILEGE };
    let plain = pg_as(false, false, Flavour::Standard);
    assert_eq!(denied(&plain, &who("alice"), Via::Standard, refused), Some(Denied::Grant(&["pg_signal_backend"])));
    let signaller = pg_as(true, false, Flavour::Standard);
    assert_eq!(denied(&signaller, &who("postgres"), Via::Standard, refused), Some(Denied::SuperuserTarget));
    let rds = pg_as(true, false, Flavour::AmazonRds);
    assert_eq!(denied(&rds, &who("rdsadmin"), Via::Standard, refused), Some(Denied::ServiceSession));
    assert_eq!(
        denied(&pg_as(false, false, Flavour::AmazonRds), &who("alice"), Via::Standard, refused),
        Some(Denied::Grant(&["rds_superuser", "pg_signal_backend"]))
    );
    assert_eq!(denied(&plain, &who("alice"), Via::Standard, NativeError::Pg { sqlstate: "57P01" }), None);
    assert_eq!(denied(&plain, &who("alice"), Via::Standard, NativeError::MySql { errno: 1095 }), None);

    let my = my_format(Flavour::Standard, 80411);
    let errno = |e| NativeError::MySql { errno: e };
    assert_eq!(
        denied(&my, &who("alice"), Via::Standard, errno(1095)),
        Some(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"]))
    );
    assert_eq!(denied(&my, &who("alice"), Via::Standard, errno(1227)), Some(Denied::SystemUserTarget));
    assert_eq!(denied(&my, &who("alice"), Via::Standard, errno(1370)), None);
    assert_eq!(denied(&my, &who("alice"), Via::Standard, errno(1317)), None);
    let rds = my_format(Flavour::AmazonRds, 80032);
    let procedure = Via::Procedure("mysql.rds_kill");
    assert_eq!(denied(&rds, &who("alice"), procedure, errno(1370)), Some(Denied::ProcedureDenied("mysql.rds_kill")));
    assert_eq!(denied(&rds, &who("rdsadmin"), Via::Standard, errno(1095)), Some(Denied::ServiceSession));
    let long = who("a_user_name_of_17c");
    assert_eq!(long.user.as_ref().unwrap().chars().count(), 18);
    assert_eq!(denied(&rds, &long, procedure, errno(1095)), Some(Denied::RdsLongUserName));
    assert_eq!(
        denied(&rds, &who("sixteen_chars_ok"), procedure, errno(1095)),
        Some(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"]))
    );
    // Fixed after 8.0.32, and the standard KILL is not affected.
    let fixed = my_format(Flavour::AmazonRds, 80033);
    assert_eq!(denied(&fixed, &long, procedure, errno(1095)), Some(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"])));
    assert_eq!(denied(&rds, &long, Via::Standard, errno(1095)), Some(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"])));
}

fn pg_target(row: &SessionRow) -> ActionTarget {
    target(&pg_format(Route::Direct), row).unwrap()
}

#[test]
fn a_cancel_is_never_called_successful_unless_the_server_says() {
    let list = pg_sessions();
    let active = by_application(&list, "zz_mon_b").clone();
    let t = pg_target(&active);
    let follow = |row: Option<SessionRow>, interrupted| FollowUp { row, interrupted };
    let cancel = |f: &FollowUp| confirm(ActionKind::Cancel, &t, f);
    assert_eq!(cancel(&follow(Some(active.clone()), None)), Confirmation::StillRunning);
    // Same statement start, but idle now: it ended.
    let ended = SessionRow { state: SessionState::Idle, ..active.clone() };
    assert_eq!(cancel(&follow(Some(ended), None)), Confirmation::EndedAfterCancel);
    let next = SessionRow { statement: Some(StatementStamp::Pg { query_start: Some(Timestamp(9)) }), ..active.clone() };
    assert_eq!(cancel(&follow(Some(next), None)), Confirmation::EndedAfterCancel);
    assert_eq!(cancel(&follow(None, None)), Confirmation::Gone);
    let replaced = SessionRow { key: SessionKey { started: Some(Timestamp(1)), ..active.key }, ..active.clone() };
    assert_eq!(cancel(&follow(Some(replaced), None)), Confirmation::Gone);
    let hidden = SessionRow { visible: Visibility::Hidden, statement: None, ..active.clone() };
    assert_eq!(cancel(&follow(Some(hidden), None)), Confirmation::Unknown);
    assert_eq!(cancel(&follow(Some(active.clone()), Some(false))), Confirmation::StillRunning);

    let (my, list) = my_root();
    let update = by_id(&list, 63114).clone();
    let t = target(&my, &update).unwrap();
    let cancel = |f: &FollowUp| confirm(ActionKind::Cancel, &t, f);
    assert_eq!(cancel(&follow(Some(update.clone()), Some(true))), Confirmation::Interrupted);
    assert_eq!(cancel(&follow(None, Some(true))), Confirmation::Interrupted);
    assert_eq!(cancel(&follow(Some(update.clone()), None)), Confirmation::StillRunning);
    let idle = SessionRow { state: SessionState::Idle, statement: None, query: None, ..update.clone() };
    assert_eq!(cancel(&follow(Some(idle), Some(false))), Confirmation::EndedAfterCancel);
    let other_thread = SessionRow { thread: Some(1), ..update.clone() };
    assert_eq!(cancel(&follow(Some(other_thread), None)), Confirmation::Gone, "the id wrapped");
}

#[test]
fn a_weak_stamp_never_confirms_still_running() {
    let (my, list) = my_unprivileged();
    let update = by_id(&list, 63114).clone();
    let t = target(&my, &update).unwrap();
    let cancel = |row: SessionRow| confirm(ActionKind::Cancel, &t, &FollowUp { row: Some(row), interrupted: None });
    assert_eq!(cancel(update.clone()), Confirmation::Unknown);
    let later = |time| SessionRow {
        statement: Some(StatementStamp::MySqlWeak {
            command: "Query".into(),
            info_md5: target(&my, &update).unwrap().stamp.and_then(|s| match s {
                StatementStamp::MySqlWeak { info_md5, .. } => info_md5,
                _ => None,
            }),
            time,
        }),
        ..update.clone()
    };
    assert_eq!(cancel(later(60)), Confirmation::Unknown, "the same text, run as long or longer");
    assert_eq!(cancel(later(1)), Confirmation::EndedAfterCancel, "a new run of the same text");
    let other = SessionRow {
        statement: Some(StatementStamp::MySqlWeak { command: "Query".into(), info_md5: Some("0".into()), time: 9 }),
        ..update.clone()
    };
    assert_eq!(cancel(other), Confirmation::EndedAfterCancel);
    let idle = SessionRow { state: SessionState::Idle, statement: None, ..update.clone() };
    assert_eq!(cancel(idle), Confirmation::EndedAfterCancel);
}

#[test]
fn an_end_is_confirmed_only_when_the_session_is_gone() {
    let list = pg_sessions();
    let idle = by_application(&list, "zz_mon_a").clone();
    let t = pg_target(&idle);
    let end = |row: Option<SessionRow>| confirm(ActionKind::Terminate, &t, &FollowUp { row, interrupted: None });
    assert_eq!(end(None), Confirmation::Gone);
    assert_eq!(end(Some(idle.clone())), Confirmation::StillThere);
    let replaced = SessionRow { key: SessionKey { started: Some(Timestamp(1)), ..idle.key }, ..idle.clone() };
    assert_eq!(end(Some(replaced)), Confirmation::Gone);
    // An interruption says nothing of an end.
    assert_eq!(
        confirm(ActionKind::Terminate, &t, &FollowUp { row: Some(idle), interrupted: Some(true) }),
        Confirmation::StillThere
    );
}

#[test]
fn a_session_answers_monitor_actions_it_does_not_do() {
    let (my, list) = my_root();
    let target = target(&my, by_id(&list, 63114)).unwrap();
    match (DbCommand::MonitorAction { request: 4, kind: ActionKind::Cancel, target: target.clone() }).not_supported() {
        DbEvent::MonitorActionDone { request: 4, outcome: ActionOutcome::Failed(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
    match (DbCommand::MonitorFollowUp { request: 5, target }).not_supported() {
        DbEvent::MonitorFollowUp { request: 5, result: Err(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_recheck_sends_only_to_the_same_session_and_statement() {
    let (my, list) = my_root();
    let update = by_id(&list, 63114).clone();
    let t = target(&my, &update).unwrap();
    let stale = |s| Some(NotSent::Stale(s));
    for kind in BOTH {
        assert_eq!(recheck(kind, &t, Some(&update)), None, "{kind:?}");
        assert_eq!(recheck(kind, &t, None), stale(Stale::SessionGone));
        let wrapped = SessionRow { thread: Some(1), ..update.clone() };
        assert_eq!(recheck(kind, &t, Some(&wrapped)), stale(Stale::SessionReplaced), "the id wrapped");
        let own = SessionRow { monitor: true, ..update.clone() };
        assert_eq!(recheck(kind, &t, Some(&own)), Some(NotSent::OwnConnection));
    }
    let next = SessionRow {
        statement: Some(StatementStamp::MySqlEvent { thread_id: 63154, event_id: 1300 }),
        ..update.clone()
    };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&next)), stale(Stale::StatementChanged));
    assert_eq!(recheck(ActionKind::Terminate, &t, Some(&next)), None, "an end needs no statement");
    let idle = SessionRow { state: SessionState::Idle, statement: None, query: None, ..update.clone() };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&idle)), stale(Stale::StatementChanged));

    // A weak stamp that matches is sent on; another text or a new run is not.
    let (my, list) = my_unprivileged();
    let weak = by_id(&list, 63114).clone();
    let t = target(&my, &weak).unwrap();
    assert!(t.stamp.as_ref().is_some_and(StatementStamp::weak));
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&weak)), None);
    let rerun = SessionRow {
        statement: t.stamp.clone().map(|s| match s {
            StatementStamp::MySqlWeak { command, info_md5, .. } => {
                StatementStamp::MySqlWeak { command, info_md5, time: 0 }
            }
            other => other,
        }),
        ..weak.clone()
    };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&rerun)), stale(Stale::StatementChanged));

    // PostgreSQL: the start of the session and of its statement.
    let list = pg_sessions();
    let active = by_application(&list, "zz_mon_b").clone();
    let t = pg_target(&active);
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&active)), None);
    let replaced = SessionRow { key: SessionKey { started: Some(Timestamp(1)), ..active.key }, ..active.clone() };
    assert_eq!(recheck(ActionKind::Terminate, &t, Some(&replaced)), stale(Stale::SessionReplaced));
    let next = SessionRow { statement: Some(StatementStamp::Pg { query_start: Some(Timestamp(9)) }), ..active.clone() };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&next)), stale(Stale::StatementChanged));
}

/// What tells a session or its statement apart, hidden since or read in another form (the
/// statement events switched on or off), is never taken for proof either way: nothing is sent,
/// and a follow-up cannot tell.
#[test]
fn identity_hidden_or_read_another_way_proves_nothing() {
    let stale = |s| Some(NotSent::Stale(s));
    let follow = |row: SessionRow| FollowUp { row: Some(row), interrupted: None };

    // PostgreSQL: the session's start hidden from the account now.
    let list = pg_sessions();
    let active = by_application(&list, "zz_mon_b").clone();
    let t = pg_target(&active);
    let hidden = SessionRow { key: SessionKey { started: None, ..active.key }, ..active.clone() };
    for kind in BOTH {
        assert_eq!(confirm(kind, &t, &follow(hidden.clone())), Confirmation::Unknown, "{kind:?}");
        assert_eq!(recheck(kind, &t, Some(&hidden)), stale(Stale::CannotTell), "{kind:?}");
    }
    // No statement start (activity not tracked, or none ran): an end is not offered, since a
    // later client of a pooler's server session would look the same.
    let untracked = SessionRow { statement: Some(StatementStamp::Pg { query_start: None }), ..active.clone() };
    for route in [Route::Direct, Route::Pooled] {
        let pg = pg_format(route);
        assert_eq!(decide(&pg, &untracked, false, ActionKind::Terminate), Decision::Refuse(Refusal::Untracked));
    }

    // MySQL: a statement event then, the weak form now (the consumer switched off).
    let (my, list) = my_root();
    let update = by_id(&list, 63114).clone();
    let t = target(&my, &update).unwrap();
    let weak_now = SessionRow {
        statement: Some(StatementStamp::MySqlWeak { command: "Query".into(), info_md5: None, time: 60 }),
        ..update.clone()
    };
    assert_eq!(confirm(ActionKind::Cancel, &t, &follow(weak_now.clone())), Confirmation::Unknown);
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&weak_now)), stale(Stale::CannotTell));
    // The thread hidden now.
    let no_thread = SessionRow { thread: None, ..update.clone() };
    for kind in BOTH {
        assert_eq!(confirm(kind, &t, &follow(no_thread.clone())), Confirmation::Unknown, "{kind:?}");
        assert_eq!(recheck(kind, &t, Some(&no_thread)), stale(Stale::CannotTell), "{kind:?}");
    }

    // MySQL: the weak form then, a statement event now (the consumer switched on): another
    // statement may run, so nothing is sent.
    let (my, list) = my_unprivileged();
    let weak = by_id(&list, 63114).clone();
    let t = target(&my, &weak).unwrap();
    let event_now =
        SessionRow { statement: Some(StatementStamp::MySqlEvent { thread_id: 1, event_id: 2 }), ..weak.clone() };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&event_now)), stale(Stale::CannotTell));
    assert_eq!(confirm(ActionKind::Cancel, &t, &follow(event_now)), Confirmation::Unknown);
}
