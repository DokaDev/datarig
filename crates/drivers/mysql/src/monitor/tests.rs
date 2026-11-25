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
        threads: Readable::Yes,
        unconfirmed: false,
        flavour,
    }
}

/// The format the monitor keeps beside `m`.
fn format_of(m: &MySqlMonitor) -> MonitorFormat {
    crate::engine::monitor_format(m.clone())
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
    Err(Fail::Server { code, error: DbError::Server(format!("ERROR {code}").into()) })
}

/// The monitor connection's marker in these tests.
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// The monitor's marker, confirmed on its own session (77, the handshake's id).
fn marker() -> Marker {
    Marker { token: TOKEN.into(), handshake: 77 }
}

/// Stands in for the connection: answers each statement with the next of `answers` (none
/// left: it never answers) and keeps what was sent. A read before an action gets the next of
/// `marked` (the reading session's id, its marker's id; the last one again once they run out)
/// after the cells of each row.
struct Fake {
    answers: VecDeque<Result<Rows, Fail>>,
    sent: Vec<String>,
    marked: VecDeque<[Cell; 2]>,
}

impl Fake {
    fn new(answers: Vec<Result<Rows, Fail>>) -> Fake {
        let confirmed = [Some("77".into()), Some("77".into())];
        Fake { answers: answers.into(), sent: Vec::new(), marked: VecDeque::from([confirmed]) }
    }
}

impl Exec for Fake {
    async fn run(&mut self, sql: &str) -> Result<Rows, Fail> {
        self.sent.push(sql.to_owned());
        let marked = sql.contains(" AS marker ");
        match self.answers.pop_front() {
            Some(Ok(rows)) if marked => {
                let marked =
                    if self.marked.len() > 1 { self.marked.pop_front().unwrap() } else { self.marked[0].clone() };
                Ok(rows
                    .into_iter()
                    .map(|mut r| {
                        r.extend(marked.clone());
                        r
                    })
                    .collect())
            }
            Some(answer) => answer,
            None => std::future::pending().await,
        }
    }
}

async fn acting(my: &MySqlMonitor, kind: ActionKind, answers: Vec<Result<Rows, Fail>>) -> (ActionOutcome, Vec<String>) {
    let mut fake = Fake::new(answers);
    let (outcome, lost) =
        within(&mut fake, ACTION_DEADLINE, my, &format_of(my), false, Some(&marker()), kind, &target()).await;
    assert_eq!(lost, None);
    (outcome, fake.sent)
}

#[tokio::test]
async fn a_read_only_profile_sends_nothing() {
    for flavour in [Flavour::Standard, Flavour::AmazonRds, Flavour::AzureDatabase] {
        for kind in [ActionKind::Cancel, ActionKind::Terminate] {
            let mut fake = Fake::new(vec![running(), server(1095), running(), ok()]);
            let (outcome, lost) = within(
                &mut fake,
                ACTION_DEADLINE,
                &my(flavour),
                &format_of(&my(flavour)),
                true,
                Some(&marker()),
                kind,
                &target(),
            )
            .await;
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
    assert_eq!(sent, [sql::guard(&m, 63114, TOKEN), "KILL QUERY 63114".to_owned()]);
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
        (stale(Stale::StatementChanged), vec![sql::guard(&m, 63114, TOKEN)])
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
        let (outcome, _) =
            within(&mut fake, ACTION_DEADLINE, &m, &format_of(&m), false, Some(&marker()), kind, &own_target).await;
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
                    sql::guard(&m, 63114, TOKEN),
                    sql::kill(kind, 63114),
                    sql::guard(&m, 63114, TOKEN),
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
        (ActionOutcome::Failed(DbError::Server("ERROR 1142".into())), vec![sql::guard(&m, 63114, TOKEN)])
    );
}

#[tokio::test(start_paused = true)]
async fn an_answer_lost_after_the_kill_is_never_taken_for_one() {
    let m = my(Flavour::Standard);
    let lost = Err(Fail::Connection(DbError::Closed));
    assert_eq!(acting(&m, ActionKind::Cancel, vec![running(), lost]).await.0, ActionOutcome::OutcomeUnknown);
    // No answer to the kill: unknown, and the connection given up.
    let mut fake = Fake::new(vec![running()]);
    let (outcome, error) =
        within(&mut fake, ACTION_DEADLINE, &m, &format_of(&m), false, Some(&marker()), ActionKind::Cancel, &target())
            .await;
    assert_eq!((outcome, error), (ActionOutcome::OutcomeUnknown, Some(DbError::NoAnswer(ACTION_DEADLINE))));
    assert_eq!(fake.sent.len(), 2);
    // No answer to the read: nothing was sent.
    let mut fake = Fake::new(Vec::new());
    let (outcome, _) =
        within(&mut fake, ACTION_DEADLINE, &m, &format_of(&m), false, Some(&marker()), ActionKind::Cancel, &target())
            .await;
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

/// A monitor connection not confirmed at open sends nothing at all.
#[tokio::test]
async fn a_monitor_not_confirmed_at_open_sends_nothing() {
    let m = my(Flavour::Standard);
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        let mut fake = Fake::new(vec![running(), ok()]);
        let (outcome, lost) =
            within(&mut fake, ACTION_DEADLINE, &m, &format_of(&m), false, None, kind, &target()).await;
        assert_eq!((outcome, lost), (ActionOutcome::NotSent(NotSent::Unconfirmed), None), "{kind:?}");
        assert!(fake.sent.is_empty(), "{:?}", fake.sent);
    }
}

/// The read before an action must find the connection's marker on its own id, the handshake's:
/// else (the marker gone, on another id, or an id of something in between) nothing is sent,
/// neither the kill nor the provider's procedure.
#[tokio::test]
async fn an_action_goes_only_where_the_read_confirmed_the_monitor() {
    let m = my(Flavour::Standard);
    for marked in [[Some("77"), None], [Some("77"), Some("78")], [Some("78"), Some("78")], [None, None]] {
        let marked = marked.map(|c| c.map(str::to_owned));
        let mut fake = Fake { marked: VecDeque::from([marked.clone()]), ..Fake::new(vec![running(), ok()]) };
        let (outcome, _) = within(
            &mut fake,
            ACTION_DEADLINE,
            &m,
            &format_of(&m),
            false,
            Some(&marker()),
            ActionKind::Cancel,
            &target(),
        )
        .await;
        assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Unconfirmed), "{marked:?}");
        assert_eq!(fake.sent, [sql::guard(&m, 63114, TOKEN)], "the read only");
    }
    // Before the provider's procedure too: the second read finds the marker elsewhere.
    let rds = my(Flavour::AmazonRds);
    let moved = [Some("77".to_owned()), Some("78".to_owned())];
    let mut fake = Fake::new(vec![running(), server(1095), running(), ok()]);
    fake.marked.push_back(moved);
    let (outcome, _) = within(
        &mut fake,
        ACTION_DEADLINE,
        &rds,
        &format_of(&rds),
        false,
        Some(&marker()),
        ActionKind::Cancel,
        &target(),
    )
    .await;
    assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Unconfirmed));
    assert!(!fake.sent.iter().any(|s| s.starts_with("CALL")), "{:?}", fake.sent);
}

/// A target known by its id alone (no thread: `performance_schema.threads` not readable) is never
/// acted on: the id may name a later session.
#[tokio::test]
async fn a_target_without_its_thread_is_never_killed() {
    let m = my(Flavour::Standard);
    let bare = ActionTarget { thread: None, ..target() };
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        let mut fake = Fake::new(vec![running(), ok()]);
        let (outcome, _) =
            within(&mut fake, ACTION_DEADLINE, &m, &format_of(&m), false, Some(&marker()), kind, &bare).await;
        assert_eq!(outcome, ActionOutcome::NotSent(NotSent::Unidentified), "{kind:?}");
        assert!(!fake.sent.iter().any(|s| s.starts_with("KILL")), "{:?}", fake.sent);
    }
}

/// What a cancel and an end send, in order, on every provider: the read of the target with the
/// monitor's marker, then `KILL QUERY` or `KILL` (and on a refusal that calls for it, the target
/// read again and the provider's procedure).
#[tokio::test]
async fn a_cancel_and_an_end_send_todays_statements() {
    let flavours = [
        Flavour::Standard,
        Flavour::AmazonRds,
        Flavour::AmazonAurora,
        Flavour::GoogleCloudSql,
        Flavour::GoogleAlloyDb,
        Flavour::AzureDatabase,
    ];
    for flavour in flavours {
        let m = my(flavour);
        for (kind, kill) in [(ActionKind::Cancel, "KILL QUERY 63114"), (ActionKind::Terminate, "KILL 63114")] {
            let (outcome, sent) = acting(&m, kind, vec![running(), ok()]).await;
            assert_eq!(outcome, ActionOutcome::Acknowledged { via: Via::Standard }, "{flavour:?} {kind:?}");
            assert_eq!(sent, [sql::guard(&m, 63114, TOKEN), kill.to_owned()], "{flavour:?} {kind:?}");
            let (_, sent) = acting(&m, kind, vec![running(), server(1095), running(), ok()]).await;
            let procedure = match (flavour, kind) {
                (Flavour::AmazonRds | Flavour::AmazonAurora, ActionKind::Cancel) => {
                    Some("CALL mysql.rds_kill_query(63114)")
                }
                (Flavour::AmazonRds | Flavour::AmazonAurora, ActionKind::Terminate) => {
                    Some("CALL mysql.rds_kill(63114)")
                }
                (Flavour::AzureDatabase, ActionKind::Cancel) => Some("CALL mysql.az_kill_query(63114)"),
                (Flavour::AzureDatabase, ActionKind::Terminate) => Some("CALL mysql.az_kill(63114)"),
                (Flavour::Standard | Flavour::GoogleCloudSql | Flavour::GoogleAlloyDb, _) => None,
            };
            let mut want = vec![sql::guard(&m, 63114, TOKEN), kill.to_owned()];
            if let Some(call) = procedure {
                want.extend([sql::guard(&m, 63114, TOKEN), call.to_owned()]);
            }
            assert_eq!(sent, want, "{flavour:?} {kind:?} refused");
        }
    }
}

/// The driver's action with the engine's monitor rules looked up as `lookup`, unchecked: the
/// driver checks them itself ([`act_with`]); on the fake, with the monitor's `marker` and on
/// `target`: what it answered and what it sent.
async fn acting_with(
    lookup: Result<&'static dyn MonitorRules, Why>,
    m: &MySqlMonitor,
    read_only: bool,
    marker: Option<&Marker>,
    kind: ActionKind,
    target: &ActionTarget,
    answers: Vec<Result<Rows, Fail>>,
) -> (ActionOutcome, Vec<String>) {
    let mut fake = Fake::new(answers);
    let outcome = act_with(|_| lookup, &mut fake, m, &format_of(m), read_only, marker, kind, target, &mut false).await;
    (outcome, fake.sent)
}

/// [`acting_with`] with the confirmed marker, on [`target`].
async fn act_lookup(
    lookup: Result<&'static dyn MonitorRules, Why>,
    m: &MySqlMonitor,
    read_only: bool,
    kind: ActionKind,
    answers: Vec<Result<Rows, Fail>>,
) -> (ActionOutcome, Vec<String>) {
    acting_with(lookup, m, read_only, Some(&marker()), kind, &target(), answers).await
}

/// Without the engine's monitor rules for the format, or with rules that do not read it,
/// nothing is sent: not the read of the target, not `KILL`. A read-only profile is told first.
#[tokio::test]
async fn an_action_without_the_engines_rules_sends_nothing() {
    use datarig_core::engine::{EngineId, Fact};
    let Fact::Known(postgres) = &EngineId::Postgres.spec().monitor else { panic!("PostgreSQL has monitor rules") };
    let m = my(Flavour::AmazonRds);
    let lookups = [
        (Err(Why::NotBuilt), NotSent::Engine(Why::NotBuilt)),
        (Err(Why::NotChecked), NotSent::Engine(Why::NotChecked)),
        (Ok(*postgres), NotSent::Engine(Why::EnginesDisagree)),
    ];
    for (lookup, why) in lookups {
        for kind in [ActionKind::Cancel, ActionKind::Terminate] {
            let (outcome, sent) = act_lookup(lookup.clone(), &m, false, kind, vec![running(), ok()]).await;
            assert_eq!(outcome, ActionOutcome::NotSent(why.clone()), "{kind:?}");
            assert!(sent.is_empty(), "{sent:?}");
        }
    }
    let (outcome, sent) = act_lookup(Err(Why::NotBuilt), &m, true, ActionKind::Cancel, vec![running(), ok()]).await;
    assert_eq!((outcome, sent), (ActionOutcome::NotSent(NotSent::ReadOnly), Vec::new()));
    // The engine's own rules: the read, then `KILL`, as before.
    let Fact::Known(mysql) = &EngineId::MySql.spec().monitor else { panic!("MySQL has monitor rules") };
    let (outcome, sent) = act_lookup(Ok(*mysql), &m, false, ActionKind::Cancel, vec![running(), ok()]).await;
    assert_eq!(outcome, ActionOutcome::Acknowledged { via: Via::Standard });
    assert_eq!(sent, [sql::guard(&m, 63114, TOKEN), "KILL QUERY 63114".to_owned()]);
}
