use super::*;
use datarig_core::monitor::action::Denied;
use datarig_core::monitor::{Flavour, Readable, SessionKey};
use std::collections::VecDeque;

fn my(flavour: Flavour) -> MySqlMonitor {
    MySqlMonitor {
        version: 80411,
        performance_schema: true,
        user: "zz_admin@%".into(),
        process: Readable::Yes,
        innodb_trx: Readable::Yes,
        metadata_locks: Readable::Yes,
        statements: Readable::Yes,
        flavour,
    }
}

const ID: i64 = 63114;
const THREAD: u64 = 63154;
const EVENT: u64 = 1207;

fn target() -> ActionTarget {
    ActionTarget {
        key: SessionKey { id: ID, started: None },
        user: Some("zz_app".into()),
        thread: Some(THREAD),
        stamp: Some(StatementStamp::MySqlEvent { thread_id: THREAD, event_id: EVENT }),
    }
}

/// A session row of the guard and follow-up reads (`decode::mysql::SESSIONS`).
fn session(id: i64, monitor: bool, command: &str, event: Option<u64>) -> Vec<Cell> {
    let s = |v: &str| Some(v.to_owned());
    let thread = if monitor { 9 } else { THREAD };
    vec![
        s("1794068771983348"),
        Some(id.to_string()),
        s("zz_app"),
        s("localhost"),
        s("shop"),
        s(command),
        s("5"),
        s(if command == "Query" { "User sleep" } else { "" }),
        if command == "Query" { s("SELECT SLEEP(60) FROM zz_t") } else { None },
        s("0"),
        s(if monitor { "1" } else { "0" }),
        None,
        None,
        None,
        None,
        s("0"),
        None,
        None,
        s("0"),
        Some(thread.to_string()),
        event.map(|e| e.to_string()),
        if command == "Query" { s("6c3b5e1e2a7d0e0a8f6f1b8c0d3e2f10") } else { None },
    ]
}

fn own() -> Vec<Cell> {
    session(77, true, "Query", Some(5))
}

/// The target running its statement, and the monitor's own session.
fn running() -> Result<Rows, Fail> {
    Ok(vec![session(ID, false, "Query", Some(EVENT)), own()])
}

fn ok() -> Result<Rows, Fail> {
    Ok(Vec::new())
}

fn server(code: u16) -> Result<Rows, Fail> {
    Err(Fail::Server { code, error: DbError::Server(format!("ERROR {code}")) })
}

/// Stands in for the connection: answers each statement with the next of `answers` (none
/// left: it never answers) and keeps what was sent.
struct Fake {
    answers: VecDeque<Result<Rows, Fail>>,
    sent: Vec<String>,
}

impl Fake {
    fn new(answers: Vec<Result<Rows, Fail>>) -> Fake {
        Fake { answers: answers.into(), sent: Vec::new() }
    }
}

impl Exec for Fake {
    async fn run(&mut self, sql: &str) -> Result<Rows, Fail> {
        self.sent.push(sql.to_owned());
        match self.answers.pop_front() {
            Some(answer) => answer,
            None => std::future::pending().await,
        }
    }
}

async fn acting(my: &MySqlMonitor, kind: ActionKind, answers: Vec<Result<Rows, Fail>>) -> (ActionOutcome, Vec<String>) {
    let mut fake = Fake::new(answers);
    let (outcome, lost) = within(&mut fake, ACTION_DEADLINE, my, false, kind, &target()).await;
    assert_eq!(lost, None);
    (outcome, fake.sent)
}

#[tokio::test]
async fn a_read_only_profile_sends_nothing() {
    for flavour in [Flavour::Standard, Flavour::AmazonRds, Flavour::AzureDatabase] {
        for kind in [ActionKind::Cancel, ActionKind::Terminate] {
            let mut fake = Fake::new(vec![running(), server(1095), running(), ok()]);
            let (outcome, lost) = within(&mut fake, ACTION_DEADLINE, &my(flavour), true, kind, &target()).await;
            assert_eq!((outcome, lost), (ActionOutcome::NotSent(NotSent::ReadOnly), None), "{flavour:?} {kind:?}");
            assert!(fake.sent.is_empty(), "{:?}", fake.sent);
        }
    }
}

#[tokio::test]
async fn the_target_is_read_right_before_the_kill() {
    let m = my(Flavour::Standard);
    let (outcome, sent) = acting(&m, ActionKind::Cancel, vec![running(), ok()]).await;
    assert_eq!(outcome, ActionOutcome::Acknowledged { via: Via::Standard });
    assert_eq!(sent, [sql::guard(&m, 63114), "KILL QUERY 63114".to_owned()]);
    let (outcome, sent) = acting(&m, ActionKind::Terminate, vec![running(), ok()]).await;
    assert_eq!(outcome, ActionOutcome::Acknowledged { via: Via::Standard });
    assert_eq!(sent[1], "KILL 63114");
}

#[tokio::test]
async fn a_changed_target_is_not_killed() {
    let m = my(Flavour::Standard);
    let stale = |s| ActionOutcome::NotSent(NotSent::Stale(s));
    let next = Ok(vec![session(ID, false, "Query", Some(EVENT + 3)), own()]);
    assert_eq!(
        acting(&m, ActionKind::Cancel, vec![next]).await,
        (stale(Stale::StatementChanged), vec![sql::guard(&m, 63114)])
    );
    let idle = Ok(vec![session(ID, false, "Sleep", None), own()]);
    assert_eq!(acting(&m, ActionKind::Cancel, vec![idle.clone()]).await.0, stale(Stale::StatementChanged));
    // An end needs the session only.
    assert_eq!(
        acting(&m, ActionKind::Terminate, vec![idle, ok()]).await.0,
        ActionOutcome::Acknowledged { via: Via::Standard }
    );
    assert_eq!(acting(&m, ActionKind::Terminate, vec![Ok(vec![own()])]).await.0, stale(Stale::SessionGone));
    let mut wrapped = session(ID, false, "Query", Some(EVENT));
    wrapped[19] = Some("1".into());
    assert_eq!(
        acting(&m, ActionKind::Terminate, vec![Ok(vec![wrapped, own()])]).await.0,
        stale(Stale::SessionReplaced)
    );
    // The session ended between the read and the kill.
    assert_eq!(acting(&m, ActionKind::Cancel, vec![running(), server(1094)]).await.0, stale(Stale::SessionGone));
}

#[tokio::test]
async fn the_monitors_own_connection_is_never_killed() {
    let m = my(Flavour::Standard);
    let own_target = ActionTarget { key: SessionKey { id: 77, started: None }, thread: Some(9), ..target() };
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        let mut fake = Fake::new(vec![Ok(vec![own()]), ok()]);
        let (outcome, _) = within(&mut fake, ACTION_DEADLINE, &m, false, kind, &own_target).await;
        assert_eq!(outcome, ActionOutcome::NotSent(NotSent::OwnConnection), "{kind:?}");
        assert_eq!(fake.sent.len(), 1, "the read only: {:?}", fake.sent);
    }
}

#[tokio::test]
async fn a_managed_server_takes_its_procedure_after_reading_the_target_again() {
    for (flavour, cancel, terminate) in [
        (Flavour::AmazonRds, "mysql.rds_kill_query", "mysql.rds_kill"),
        (Flavour::AmazonAurora, "mysql.rds_kill_query", "mysql.rds_kill"),
        (Flavour::AzureDatabase, "mysql.az_kill_query", "mysql.az_kill"),
    ] {
        let m = my(flavour);
        for (kind, procedure) in [(ActionKind::Cancel, cancel), (ActionKind::Terminate, terminate)] {
            let (outcome, sent) = acting(&m, kind, vec![running(), server(1095), running(), ok()]).await;
            assert_eq!(outcome, ActionOutcome::Acknowledged { via: Via::Procedure(procedure) }, "{flavour:?}");
            assert_eq!(
                sent,
                [
                    sql::guard(&m, 63114),
                    sql::kill(kind, 63114),
                    sql::guard(&m, 63114),
                    format!("CALL {procedure}(63114)")
                ]
            );
            // Changed in between: no call.
            let gone = Ok(vec![own()]);
            let (outcome, sent) = acting(&m, kind, vec![running(), server(1095), gone]).await;
            assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Stale(Stale::SessionGone)));
            assert_eq!(sent.len(), 3, "{sent:?}");
            // The procedure refused.
            let (outcome, _) = acting(&m, kind, vec![running(), server(1095), running(), server(1370)]).await;
            assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Refused(Denied::ProcedureDenied(procedure))));
        }
        // Another refusal of the standard statement does not call for the procedure.
        let (outcome, sent) = acting(&m, ActionKind::Cancel, vec![running(), server(1227)]).await;
        assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Refused(Denied::SystemUserTarget)));
        assert_eq!(sent.len(), 2);
    }
}

#[tokio::test]
async fn a_self_managed_refusal_names_the_privilege() {
    let m = my(Flavour::Standard);
    let (outcome, sent) = acting(&m, ActionKind::Cancel, vec![running(), server(1095)]).await;
    assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Refused(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"]))));
    assert_eq!(sent.len(), 2, "no procedure: {sent:?}");
    assert_eq!(
        acting(&m, ActionKind::Terminate, vec![running(), server(1227)]).await.0,
        ActionOutcome::NotSent(NotSent::Refused(Denied::SystemUserTarget))
    );
    assert_eq!(
        acting(&m, ActionKind::Cancel, vec![running(), server(1064)]).await.0,
        ActionOutcome::Failed(DbError::Server("ERROR 1064".into()))
    );
    // A failed read sends nothing.
    assert_eq!(
        acting(&m, ActionKind::Cancel, vec![server(1142)]).await,
        (ActionOutcome::Failed(DbError::Server("ERROR 1142".into())), vec![sql::guard(&m, 63114)])
    );
}

#[tokio::test(start_paused = true)]
async fn an_answer_lost_after_the_kill_is_never_taken_for_one() {
    let m = my(Flavour::Standard);
    let lost = Err(Fail::Connection(DbError::Closed));
    assert_eq!(acting(&m, ActionKind::Cancel, vec![running(), lost]).await.0, ActionOutcome::OutcomeUnknown);
    // No answer to the kill: unknown, and the connection given up.
    let mut fake = Fake::new(vec![running()]);
    let (outcome, error) = within(&mut fake, ACTION_DEADLINE, &m, false, ActionKind::Cancel, &target()).await;
    assert_eq!((outcome, error), (ActionOutcome::OutcomeUnknown, Some(DbError::NoAnswer(ACTION_DEADLINE))));
    assert_eq!(fake.sent.len(), 2);
    // No answer to the read: nothing was sent.
    let mut fake = Fake::new(Vec::new());
    let (outcome, _) = within(&mut fake, ACTION_DEADLINE, &m, false, ActionKind::Cancel, &target()).await;
    assert_eq!(outcome, ActionOutcome::Failed(DbError::NoAnswer(ACTION_DEADLINE)));
}

#[tokio::test]
async fn a_follow_up_reads_whether_the_statement_was_interrupted() {
    let m = my(Flavour::Standard);
    let errno = |e: &str| Ok(vec![vec![Some(e.to_owned())]]);
    for (answer, interrupted) in
        [(errno("1317"), Some(true)), (errno("0"), Some(false)), (Ok(Vec::new()), None), (server(1142), None)]
    {
        let mut fake = Fake::new(vec![Ok(vec![session(ID, false, "Sleep", None), own()]), answer]);
        let follow = follow_up(&mut fake, &m, &target()).await.expect("follow-up");
        assert_eq!(follow.interrupted, interrupted);
        assert_eq!(follow.row.map(|r| r.key.id), Some(ID));
        assert_eq!(fake.sent, [sql::session(&m, 63114), sql::statement_errno(THREAD, EVENT)]);
    }
    // A weak stamp names no event: nothing more is read.
    let weak = ActionTarget {
        thread: None,
        stamp: Some(StatementStamp::MySqlWeak { command: "Query".into(), info_md5: None, time: 5 }),
        ..target()
    };
    let mut fake = Fake::new(vec![Ok(vec![own()])]);
    let follow = follow_up(&mut fake, &m, &weak).await.expect("follow-up");
    assert_eq!((follow.row, follow.interrupted, fake.sent.len()), (None, None, 1));
}
