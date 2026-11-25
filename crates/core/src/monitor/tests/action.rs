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
        assert_eq!(decide(&my, by_id(&my_list, 18986), true, kind), Decision::Refuse(Refusal::ReadOnly));
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
    // 18986 runs a statement event (a strong stamp).
    let read = my_row(&list, 18986, "SELECT id, v FROM zz_mon_t WHERE id = 1", false);
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
        // The monitor does not know which engine of the family the server is: a statement
        // that takes a sequence's next value on MariaDB is no plain read.
        "SELECT NEXT VALUE FOR zz_s",
        "SELECT zz_s.nextval",
        "SELECT NEXTVAL(zz_s)",
    ] {
        let row = my_row(&list, 18986, text, false);
        assert_eq!(decide(&my, &row, false, ActionKind::Cancel), half, "{text}");
    }
    // A read with a MySQL built-in that MariaDB lacks is still a plain read on the monitor.
    let read = my_row(&list, 18986, "SELECT BIN_TO_UUID(id) FROM zz_mon_t WHERE id = 1", false);
    assert_eq!(decide(&my, &read, false, ActionKind::Cancel), LINE);
    // A read the list holds only the start of cannot be judged.
    let cut = by_id(&list, 18993);
    assert!(cut.query.as_ref().unwrap().shortened);
    assert_eq!(decide(&my, cut, false, ActionKind::Cancel), half);
    for text in
        ["ALTER TABLE t ADD COLUMN x INT", "optimize table t", " (CHECK TABLE t)", "LOAD DATA INFILE 'f' INTO TABLE t"]
    {
        let row = my_row(&list, 18986, text, true);
        assert_eq!(decide(&my, &row, false, ActionKind::Cancel), offer(&[Warning::MySqlMaintenance]), "{text}");
    }
    let no_text = SessionRow { query: None, ..by_id(&list, 18986).clone() };
    assert_eq!(decide(&my, &no_text, false, ActionKind::Cancel), half, "unknown is not a read");
    assert_eq!(decide(&my, by_id(&list, 18985), false, ActionKind::Cancel), Decision::Refuse(Refusal::NoStatement));
}

#[test]
fn a_weak_mysql_stamp_is_said_so() {
    let (my, list) = my_root();
    // The ALTER's thread is not instrumented: its statement has no event.
    let alter = by_id(&list, 18990);
    assert!(alter.statement.as_ref().is_some_and(StatementStamp::weak));
    assert_eq!(
        decide(&my, alter, false, ActionKind::Cancel),
        offer(&[Warning::WeakIdentity, Warning::MySqlMaintenance])
    );
    let (my, list) = my_unprivileged();
    let read = my_row(&list, 18986, "SELECT 1", false);
    assert_eq!(decide(&my, &read, false, ActionKind::Cancel), offer(&[Warning::WeakIdentity]));
}

#[test]
fn ending_a_mysql_session_says_non_transactional_changes_stay() {
    let (my, list) = my_root();
    for id in [18985, 18986, 18990] {
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
    let update = target(&my, by_id(&list, 18986)).unwrap();
    assert_eq!(update.key, SessionKey { id: 18986, started: None });
    assert_eq!(update.thread, Some(19035));
    assert_eq!(update.stamp, Some(StatementStamp::MySqlEvent { thread_id: 19035, event_id: 4 }));
    let alter = target(&my, by_id(&list, 18990)).unwrap();
    assert_eq!(
        alter.stamp,
        Some(StatementStamp::MySqlWeak {
            command: "Query".into(),
            info_md5: Some("b7b2a88be5a2ced1d67a6f7293da518d".into()),
            time: 5
        })
    );
    // Idle: no statement runs, so there is none to name.
    assert_eq!(target(&my, by_id(&list, 18985)).unwrap().stamp, None);
    assert_eq!(by_id(&list, 5).statement, None);

    let (my, list) = my_unprivileged();
    let weak = target(&my, by_id(&list, 18986)).unwrap();
    assert_eq!(weak.thread, None);
    assert!(matches!(weak.stamp, Some(StatementStamp::MySqlWeak { ref command, time: 9, .. }) if command == "Query"));
    let m = decode::mysql::probe(&fixture!("my80/probe")).unwrap();
    let (list, _) = decode::mysql::sessions(&rows(fixture!("my80/sessions")), &m, IDLE).unwrap();
    assert_eq!(by_id(&list, 14).statement, Some(StatementStamp::MySqlEvent { thread_id: 54, event_id: 2 }));
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
    let update = by_id(&list, 18986).clone();
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
    let update = by_id(&list, 18986).clone();
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
    let target = target(&my, by_id(&list, 18986)).unwrap();
    match (DbCommand::MonitorAction { request: 4, kind: ActionKind::Cancel, target: target.clone() }).not_supported() {
        DbEvent::MonitorActionDone { request: 4, outcome: ActionOutcome::Failed(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
    match (DbCommand::MonitorFollowUp { request: 5, target }).not_supported() {
        DbEvent::MonitorFollowUp { request: 5, result: Err(DbError::NotSupported) } => {}
        other => panic!("{other:?}"),
    }
}

/// A MySQL target that carries neither its start nor its thread is never acted on: MySQL gives
/// a later session the same id while it runs. The account is told what would show the thread.
#[test]
fn a_target_known_by_its_id_alone_is_never_acted_on() {
    let (my, list) = my_unprivileged();
    let row = by_id(&list, 18986).clone();
    let t = target(&my, &SessionRow { thread: None, ..row.clone() }).unwrap();
    assert_eq!((t.key.started, t.thread), (None, None));
    for kind in BOTH {
        assert_eq!(recheck(kind, &t, Some(&row)), Some(NotSent::Unidentified), "{kind:?}");
        assert_eq!(recheck(kind, &t, None), Some(NotSent::Unidentified), "{kind:?}");
    }
    // With the thread known (performance_schema.threads readable) it is.
    let row = SessionRow { thread: Some(19035), ..row };
    let t = target(&my, &row).unwrap();
    assert_eq!(recheck(ActionKind::Terminate, &t, Some(&row)), None);
}

/// Another account shown under the target's id and thread (a later session, or the same one
/// inside a `SQL SECURITY DEFINER` routine on MySQL before 8.0.22): nothing is sent, and a
/// follow-up does not call the session gone, as its thread is the same.
#[test]
fn another_account_on_the_same_thread_sends_nothing_and_is_not_gone() {
    let (my, list) = my_root();
    let update = by_id(&list, 18986).clone();
    let t = target(&my, &update).unwrap();
    assert!(t.user.is_some() && t.thread.is_some());
    let other = SessionRow { user: Some("zz_someone_else".into()), ..update.clone() };
    assert_eq!(other.thread, update.thread);
    let unknown = SessionRow { user: None, ..update.clone() };
    for kind in BOTH {
        assert_eq!(recheck(kind, &t, Some(&other)), Some(NotSent::Stale(Stale::CannotTell)), "{kind:?}");
        assert_eq!(recheck(kind, &t, Some(&unknown)), recheck(kind, &t, Some(&update)), "{kind:?}");
        let follow = FollowUp { row: Some(other.clone()), interrupted: None };
        assert_ne!(confirm(kind, &t, &follow), Confirmation::Gone, "{kind:?}");
    }
    assert_eq!(
        confirm(ActionKind::Terminate, &t, &FollowUp { row: Some(other), interrupted: None }),
        Confirmation::StillThere
    );
}

/// A MySQL monitor connection not confirmed when it opened offers no action on any row.
#[test]
fn an_unconfirmed_monitor_offers_no_action() {
    let (my, list) = my_root();
    let update = by_id(&list, 18986).clone();
    let MonitorFormat::MySql(my) = my else { unreachable!("a MySQL format") };
    let format = |unconfirmed| MonitorFormat::MySql(MySqlMonitor { unconfirmed, ..my.clone() });
    for kind in BOTH {
        assert!(matches!(decide(&format(false), &update, false, kind), Decision::Offer(_)), "{kind:?}");
        assert_eq!(decide(&format(true), &update, false, kind), Decision::Refuse(Refusal::Unconfirmed), "{kind:?}");
    }
}

#[test]
fn a_recheck_sends_only_to_the_same_session_and_statement() {
    let (my, list) = my_root();
    let update = by_id(&list, 18986).clone();
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
        statement: Some(StatementStamp::MySqlEvent { thread_id: 19035, event_id: 1300 }),
        ..update.clone()
    };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&next)), stale(Stale::StatementChanged));
    assert_eq!(recheck(ActionKind::Terminate, &t, Some(&next)), None, "an end needs no statement");
    let idle = SessionRow { state: SessionState::Idle, statement: None, query: None, ..update.clone() };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&idle)), stale(Stale::StatementChanged));

    // A weak stamp that matches is sent on; another text or a new run is not.
    let (my, list) = my_unprivileged();
    // The statement events refused, `performance_schema.threads` readable: the thread is known.
    let weak = SessionRow { thread: Some(19035), ..by_id(&list, 18986).clone() };
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
    let update = by_id(&list, 18986).clone();
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
    // The statement events refused, `performance_schema.threads` readable: the thread is known.
    let weak = SessionRow { thread: Some(19035), ..by_id(&list, 18986).clone() };
    let t = target(&my, &weak).unwrap();
    let event_now =
        SessionRow { statement: Some(StatementStamp::MySqlEvent { thread_id: 1, event_id: 2 }), ..weak.clone() };
    assert_eq!(recheck(ActionKind::Cancel, &t, Some(&event_now)), stale(Stale::CannotTell));
    assert_eq!(confirm(ActionKind::Cancel, &t, &follow(event_now)), Confirmation::Unknown);
}

/// One row of [`every_decision_answers_as_today`]: what `decide` answers for a cancel and for
/// an end of `row` on a server read with `format`.
struct DecisionCase {
    name: &'static str,
    format: MonitorFormat,
    row: SessionRow,
    read_only: bool,
    cancel: Decision,
    terminate: Decision,
}

/// Every branch of `decide` on both formats, for both actions, as the base answers them: each
/// condition alone, then the conditions that overlap, whose order decides which refusal wins.
#[test]
fn every_decision_answers_as_today() {
    let refuse = Decision::Refuse;
    let hidden = refuse(Refusal::Hidden(Need::Grant("pg_read_all_stats")));
    let pg_list = pg_sessions();
    let pg_running = by_application(&pg_list, "zz_mon_b").clone();
    assert_eq!((&pg_running.state, &pg_running.visible), (&SessionState::Active, &Visibility::Full));
    assert!(matches!(&pg_running.statement, Some(StatementStamp::Pg { query_start: Some(_) })));
    let pg_idle = SessionRow { state: SessionState::Idle, ..pg_running.clone() };
    let pg_untracked = SessionRow {
        query_started: None,
        statement: Some(StatementStamp::Pg { query_start: None }),
        ..pg_idle.clone()
    };
    let pg_no_stamp = SessionRow { statement: None, ..pg_running.clone() };
    let pg_hidden = SessionRow { visible: Visibility::Hidden, ..pg_running.clone() };
    let pg_no_start = SessionRow { key: SessionKey { started: None, ..pg_running.key }, ..pg_running.clone() };
    let pg_own = SessionRow { monitor: true, ..pg_running.clone() };
    let pg_background = by_id(&pg_list, 26).clone();
    assert!(matches!(pg_background.kind, SessionKind::Background(_)));
    let pg_replication = SessionRow { kind: SessionKind::Replication, ..pg_running.clone() };
    let pg_worker = SessionRow { kind: SessionKind::ParallelWorker { leader: Some(1) }, ..pg_running.clone() };
    let pg_background_idle = SessionRow { kind: SessionKind::Replication, ..pg_idle.clone() };

    let (my, my_list) = my_root();
    let MonitorFormat::MySql(m) = &my else { unreachable!() };
    let my_unconfirmed = MonitorFormat::MySql(MySqlMonitor { unconfirmed: true, ..m.clone() });
    // A plain read on a strong stamp, then the same on a weak one.
    let my_running = my_row(&my_list, 18986, "SELECT id, v FROM zz_mon_t WHERE id = 1", false);
    assert!(matches!(&my_running.statement, Some(StatementStamp::MySqlEvent { .. })));
    let my_weak = my_row(&my_list, 18990, "SELECT id, v FROM zz_mon_t WHERE id = 1", false);
    assert!(my_weak.statement.as_ref().is_some_and(StatementStamp::weak));
    let my_idle = by_id(&my_list, 18985).clone();
    assert_eq!(my_idle.statement, None);
    let my_no_stamp = SessionRow { statement: None, ..my_running.clone() };
    let my_hidden = SessionRow { visible: Visibility::Hidden, ..my_running.clone() };
    let my_own = SessionRow { monitor: true, ..my_running.clone() };
    let my_background = by_id(&my_list, 5).clone();
    assert!(matches!(my_background.kind, SessionKind::Background(_)));

    let direct = pg_format(Route::Direct);
    let pooled = pg_format(Route::Pooled);
    let unknown = pg_format(Route::Unknown);
    let end = offer(&[]);
    let shared = offer(&[Warning::PoolerSharedBackend]);
    let keeps = offer(&[Warning::MySqlTerminateKeepsNonTx]);
    let case = |name, format: &MonitorFormat, row: &SessionRow, read_only, cancel, terminate| DecisionCase {
        name,
        format: format.clone(),
        row: row.clone(),
        read_only,
        cancel,
        terminate,
    };
    let cases = [
        // PostgreSQL, each condition alone.
        case("pg read-only", &direct, &pg_running, true, refuse(Refusal::ReadOnly), refuse(Refusal::ReadOnly)),
        case(
            "pg own connection",
            &direct,
            &pg_own,
            false,
            refuse(Refusal::MonitorConnection),
            refuse(Refusal::MonitorConnection),
        ),
        case("pg hidden", &direct, &pg_hidden, false, hidden.clone(), hidden.clone()),
        case("pg start hidden", &direct, &pg_no_start, false, hidden.clone(), hidden.clone()),
        case("pg background", &direct, &pg_background, false, refuse(Refusal::Background), refuse(Refusal::Background)),
        case(
            "pg replication",
            &direct,
            &pg_replication,
            false,
            refuse(Refusal::Background),
            refuse(Refusal::Background),
        ),
        case("pg worker", &direct, &pg_worker, false, refuse(Refusal::Background), refuse(Refusal::Background)),
        case("pg idle", &direct, &pg_idle, false, refuse(Refusal::NoStatement), end.clone()),
        case("pg running", &direct, &pg_running, false, LINE, end.clone()),
        case("pg untracked", &direct, &pg_untracked, false, refuse(Refusal::NoStatement), refuse(Refusal::Untracked)),
        case("pg no stamp", &direct, &pg_no_stamp, false, refuse(Refusal::NoStatement), refuse(Refusal::Untracked)),
        case("pg pooled idle", &pooled, &pg_idle, false, refuse(Refusal::NoStatement), refuse(Refusal::PooledIdle)),
        case("pg pooled running", &pooled, &pg_running, false, LINE, shared.clone()),
        case(
            "pg unknown route idle",
            &unknown,
            &pg_idle,
            false,
            refuse(Refusal::NoStatement),
            refuse(Refusal::PooledIdle),
        ),
        case("pg unknown route running", &unknown, &pg_running, false, LINE, shared.clone()),
        // MySQL, each condition alone.
        case("my read-only", &my, &my_running, true, refuse(Refusal::ReadOnly), refuse(Refusal::ReadOnly)),
        case(
            "my own connection",
            &my,
            &my_own,
            false,
            refuse(Refusal::MonitorConnection),
            refuse(Refusal::MonitorConnection),
        ),
        case(
            "my unconfirmed",
            &my_unconfirmed,
            &my_running,
            false,
            refuse(Refusal::Unconfirmed),
            refuse(Refusal::Unconfirmed),
        ),
        case("my hidden", &my, &my_hidden, false, LINE, keeps.clone()),
        case("my background", &my, &my_background, false, refuse(Refusal::Background), refuse(Refusal::Background)),
        case("my idle", &my, &my_idle, false, refuse(Refusal::NoStatement), keeps.clone()),
        case("my running strong", &my, &my_running, false, LINE, keeps.clone()),
        case("my running weak", &my, &my_weak, false, offer(&[Warning::WeakIdentity]), keeps.clone()),
        case("my no stamp", &my, &my_no_stamp, false, refuse(Refusal::NoStatement), keeps.clone()),
        // Conditions that overlap: the first in `decide`'s order wins.
        case("pg read-only own", &direct, &pg_own, true, refuse(Refusal::ReadOnly), refuse(Refusal::ReadOnly)),
        case("my read-only own", &my, &my_own, true, refuse(Refusal::ReadOnly), refuse(Refusal::ReadOnly)),
        case(
            "my read-only unconfirmed",
            &my_unconfirmed,
            &my_running,
            true,
            refuse(Refusal::ReadOnly),
            refuse(Refusal::ReadOnly),
        ),
        case(
            "my own unconfirmed",
            &my_unconfirmed,
            &my_own,
            false,
            refuse(Refusal::MonitorConnection),
            refuse(Refusal::MonitorConnection),
        ),
        case(
            "my unconfirmed hidden",
            &my_unconfirmed,
            &my_hidden,
            false,
            refuse(Refusal::Unconfirmed),
            refuse(Refusal::Unconfirmed),
        ),
        case(
            "pg own hidden",
            &direct,
            &SessionRow { monitor: true, ..pg_hidden.clone() },
            false,
            refuse(Refusal::MonitorConnection),
            refuse(Refusal::MonitorConnection),
        ),
        case(
            "pg hidden background",
            &direct,
            &SessionRow { kind: SessionKind::Replication, ..pg_hidden.clone() },
            false,
            hidden.clone(),
            hidden.clone(),
        ),
        case(
            "pg background no statement",
            &direct,
            &pg_background_idle,
            false,
            refuse(Refusal::Background),
            refuse(Refusal::Background),
        ),
        case(
            "my background no statement",
            &my,
            &SessionRow { statement: None, ..my_background.clone() },
            false,
            refuse(Refusal::Background),
            refuse(Refusal::Background),
        ),
        case(
            "pg untracked pooled idle",
            &pooled,
            &pg_untracked,
            false,
            refuse(Refusal::NoStatement),
            refuse(Refusal::Untracked),
        ),
        case(
            "pg untracked unknown route idle",
            &unknown,
            &pg_untracked,
            false,
            refuse(Refusal::NoStatement),
            refuse(Refusal::Untracked),
        ),
    ];
    for c in &cases {
        assert_eq!(decide(&c.format, &c.row, c.read_only, ActionKind::Cancel), c.cancel, "{}: cancel", c.name);
        assert_eq!(decide(&c.format, &c.row, c.read_only, ActionKind::Terminate), c.terminate, "{}: end", c.name);
    }
}

/// The warnings of a MySQL cancel and their order, for texts on a strong and on a weak stamp.
#[test]
fn mysql_cancel_warnings_answer_as_today() {
    use Warning::{MySqlMaintenance as Maintenance, MySqlMayLeaveHalfDone as HalfDone, WeakIdentity as Weak};
    let (my, list) = my_root();
    // (text, shortened, warnings on a strong stamp; a weak one adds `WeakIdentity` first).
    let cases: [(&str, bool, &[Warning]); 8] = [
        ("SELECT id FROM zz_mon_t", false, &[]),
        // A sibling read calls it a write (a sequence's next value on MariaDB).
        ("SELECT NEXTVAL(zz_s)", false, &[HalfDone]),
        ("SELECT zz_s.nextval", false, &[HalfDone]),
        // A maintenance word after `(`, whole or shortened.
        ("(ALTER TABLE zz_mon_t ENGINE=InnoDB)", false, &[Maintenance]),
        ("  (OPTIMIZE TABLE zz_mon_t", true, &[Maintenance]),
        // A plain read shortened: it cannot be judged.
        ("SELECT id FROM zz_mon_t", true, &[HalfDone]),
        ("UPDATE zz_mon_t SET v = 1", false, &[HalfDone]),
        ("EXPLAIN ANALYZE SELECT 1", false, &[HalfDone]),
    ];
    for (text, shortened, strong) in cases {
        let row = my_row(&list, 18986, text, shortened);
        let want = if strong.is_empty() { LINE } else { offer(strong) };
        assert_eq!(decide(&my, &row, false, ActionKind::Cancel), want, "strong {text:?} {shortened}");
        let row = my_row(&list, 18990, text, shortened);
        let weak: Vec<Warning> = std::iter::once(Weak).chain(strong.iter().copied()).collect();
        assert_eq!(decide(&my, &row, false, ActionKind::Cancel), offer(&weak), "weak {text:?} {shortened}");
    }
}

/// What every refusal of the server means, on both formats: every flavour, the standard
/// statement and a procedure, PostgreSQL's privilege error, MySQL's three and an unrelated one of
/// each, a service account, a long and a short user name, an account that may signal or not
/// (PostgreSQL) and Amazon RDS 8.0.32 and 8.0.33 (MySQL). Recorded in
/// `tests/golden/refusal_readings.txt` (`DATARIG_BLESS=1` writes it anew).
#[test]
fn every_refusal_reading_answers_as_today() {
    use std::fmt::Write as _;
    let errors = [
        NativeError::Pg { sqlstate: PG_INSUFFICIENT_PRIVILEGE },
        NativeError::Pg { sqlstate: "57P01" },
        NativeError::MySql { errno: MYSQL_KILL_DENIED },
        NativeError::MySql { errno: MYSQL_NEEDS_PRIVILEGE },
        NativeError::MySql { errno: MYSQL_EXECUTE_DENIED },
        NativeError::MySql { errno: 1317 },
    ];
    let users = ["rdsadmin", "azure_superuser", "a_user_name_of_17c", "alice"];
    let vias = [Via::Standard, Via::Procedure("mysql.rds_kill")];
    let mut formats = Vec::new();
    for flavour in FLAVOURS {
        for (signal_backend, superuser) in [(false, false), (true, false), (false, true)] {
            let pg = crate::monitor::PgMonitor { signal_backend, superuser, flavour, ..pg(170011) };
            formats.push((
                format!("pg {flavour:?} signal {signal_backend} super {superuser}"),
                MonitorFormat::Postgres(pg),
            ));
        }
        for version in [80032, 80033] {
            formats.push((format!("my {flavour:?} {version}"), my_format(flavour, version)));
        }
    }
    let mut got = String::new();
    for (name, format) in &formats {
        for via in vias {
            for user in users {
                let answers: Vec<String> =
                    errors.iter().map(|e| format!("{e:?} {:?}", denied(format, &who(user), via, *e))).collect();
                let _ = writeln!(got, "{name} | {via:?} | {user}: {}", answers.join(", "));
            }
        }
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/monitor/tests/golden/refusal_readings.txt");
    if std::env::var("DATARIG_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &got).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (got, want): (Vec<&str>, Vec<&str>) = (got.lines().collect(), want.lines().collect());
    for (n, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "line {} (DATARIG_BLESS=1 cargo test -p datarig-core --lib refusal_reading)", n + 1);
    }
    assert_eq!(got.len(), want.len());
}
