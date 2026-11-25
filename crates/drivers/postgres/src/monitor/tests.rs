use super::*;
use datarig_core::monitor::action::Denied;
use datarig_core::monitor::{Flavour, SessionKey, StatementStamp, Timestamp};
use std::sync::Mutex;

fn pg(route: Route) -> PgMonitor {
    PgMonitor {
        version: 170011,
        superuser: false,
        read_all_stats: true,
        signal_backend: false,
        flavour: Flavour::Standard,
        route,
    }
}

/// The format the monitor keeps beside `m`.
fn format_of(m: &PgMonitor) -> MonitorFormat {
    crate::engine::monitor_format(m.clone())
}

fn target() -> ActionTarget {
    ActionTarget {
        key: SessionKey { id: 4242, started: Some(Timestamp(1_794_068_700_000_001)) },
        user: Some("zz_app".into()),
        thread: None,
        stamp: Some(StatementStamp::Pg { query_start: Some(Timestamp(1_794_068_760_000_002)) }),
    }
}

/// Stands in for the connection: answers each transaction with the next of `answers` and keeps
/// what was sent.
struct Fake {
    answers: Mutex<Vec<Option<Pipelined>>>,
    sent: Mutex<Vec<String>>,
}

impl Fake {
    fn new(answers: Vec<Option<Pipelined>>) -> Fake {
        Fake { answers: Mutex::new(answers.into_iter().rev().collect()), sent: Mutex::new(Vec::new()) }
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Pipeline for Fake {
    async fn send(&self, begin: &str, statement: &str, _deadline: Duration) -> Option<Pipelined> {
        self.sent.lock().unwrap().extend([begin.to_owned(), statement.to_owned(), "COMMIT".to_owned()]);
        self.answers.lock().unwrap().pop().expect("an answer for every transaction")
    }
}

fn row(own: &str, session: &str, statement: &str, sent: Option<&str>) -> Rows {
    tracked_row(own, session, "t", statement, sent)
}

fn tracked_row(own: &str, session: &str, tracked: &str, statement: &str, sent: Option<&str>) -> Rows {
    let flag = |f: &str| Some(f.to_owned());
    vec![vec![flag(own), flag(session), flag(tracked), flag(statement), sent.map(str::to_owned)]]
}

fn answered(acted: Result<Rows, Fail>) -> Option<Pipelined> {
    Some(Pipelined { begun: Ok(()), acted, committed: Ok(true) })
}

fn server(sqlstate: &str) -> Fail {
    Fail::Server { sqlstate: sqlstate.into(), error: DbError::Server(format!("ERROR: {sqlstate}").into()) }
}

async fn acting(pg: &PgMonitor, kind: ActionKind, answer: Option<Pipelined>) -> Acted {
    act(&Fake::new(vec![answer]), pg, &format_of(pg), false, kind, &target()).await
}

#[tokio::test]
async fn a_read_only_profile_sends_nothing() {
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        for route in [Route::Direct, Route::Pooled, Route::Unknown] {
            let fake = Fake::new(Vec::new());
            let acted = act(&fake, &pg(route), &format_of(&pg(route)), true, kind, &target()).await;
            assert_eq!(acted.outcome, ActionOutcome::NotSent(NotSent::ReadOnly), "{kind:?} {route:?}");
            assert!(fake.sent().is_empty(), "{:?}", fake.sent());
            assert!(!acted.committed && acted.lost.is_none());
        }
    }
}

#[tokio::test]
async fn an_action_is_one_read_only_transaction_of_its_own() {
    let fake = Fake::new(vec![answered(Ok(row("f", "t", "t", Some("t"))))]);
    let acted =
        act(&fake, &pg(Route::Direct), &format_of(&pg(Route::Direct)), false, ActionKind::Cancel, &target()).await;
    assert_eq!(
        acted,
        Acted { outcome: ActionOutcome::Acknowledged { via: Via::Standard }, committed: true, lost: None }
    );
    let sent = fake.sent();
    assert_eq!(sent.len(), 3);
    assert!(sent[0].starts_with("BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY NOT DEFERRABLE;"), "{}", sent[0]);
    assert_eq!(sent[0], sql::begin(ACTION_LIMIT), "the tick's settings");
    assert_eq!(sent[1], sql::action(&pg(Route::Direct), ActionKind::Cancel, &target()));
    assert_eq!(sent[2], "COMMIT");
}

#[tokio::test]
async fn the_servers_answer_is_told_as_it_is() {
    let direct = pg(Route::Direct);
    let cancel = ActionKind::Cancel;
    let stale = |s| ActionOutcome::NotSent(NotSent::Stale(s));
    assert_eq!(
        acting(&direct, cancel, answered(Ok(row("f", "t", "t", Some("f"))))).await.outcome,
        ActionOutcome::NotDelivered
    );
    assert_eq!(acting(&direct, cancel, answered(Ok(Vec::new()))).await.outcome, stale(Stale::SessionGone));
    assert_eq!(
        acting(&direct, cancel, answered(Ok(row("f", "f", "t", None)))).await.outcome,
        stale(Stale::SessionReplaced)
    );
    assert_eq!(
        acting(&direct, cancel, answered(Ok(row("f", "t", "f", None)))).await.outcome,
        stale(Stale::StatementChanged)
    );
    assert_eq!(
        acting(&direct, cancel, answered(Ok(row("t", "t", "t", None)))).await.outcome,
        ActionOutcome::NotSent(NotSent::OwnConnection)
    );
    // An end needs the same last statement too (the statement's flag says whether it must
    // still run: behind a pooler, or where that is not known).
    let end = ActionKind::Terminate;
    assert_eq!(
        acting(&direct, end, answered(Ok(row("f", "t", "t", Some("t"))))).await.outcome,
        ActionOutcome::Acknowledged { via: Via::Standard }
    );
    assert_eq!(
        acting(&direct, end, answered(Ok(row("f", "t", "f", None)))).await.outcome,
        stale(Stale::StatementChanged)
    );
    for route in [Route::Pooled, Route::Unknown] {
        assert_eq!(
            acting(&pg(route), end, answered(Ok(row("f", "t", "f", None)))).await.outcome,
            stale(Stale::StatementChanged),
            "{route:?}"
        );
    }
    // Activity not tracked (`track_activities` off): a NULL start proves nothing.
    for kind in [cancel, end] {
        assert_eq!(
            acting(&direct, kind, answered(Ok(tracked_row("f", "t", "f", "t", None)))).await.outcome,
            stale(Stale::CannotTell)
        );
    }
    // All held and no answer from the function: not taken for either.
    assert_eq!(
        acting(&direct, cancel, answered(Ok(row("f", "t", "t", None)))).await.outcome,
        ActionOutcome::OutcomeUnknown
    );
}

#[tokio::test]
async fn the_answer_stands_whatever_becomes_of_the_commit() {
    let lost = Fail::Connection(DbError::Closed);
    let answer = Pipelined { begun: Ok(()), acted: Ok(row("f", "t", "t", Some("t"))), committed: Err(lost.clone()) };
    let acted = acting(&pg(Route::Direct), ActionKind::Cancel, Some(answer)).await;
    assert_eq!(acted.outcome, ActionOutcome::Acknowledged { via: Via::Standard });
    assert!(!acted.committed, "not counted as one of the connection's transactions");
    // A refused statement aborts the transaction: its COMMIT answers ROLLBACK, and is not
    // counted (the server's commit count did not move).
    let answer = Pipelined { begun: Ok(()), acted: Err(server("42501")), committed: Ok(false) };
    assert!(!acting(&pg(Route::Direct), ActionKind::Cancel, Some(answer)).await.committed, "rolled back");
    assert!(
        acting(&pg(Route::Direct), ActionKind::Cancel, answered(Ok(row("f", "t", "t", Some("t"))))).await.committed
    );
    let answer = Pipelined { begun: Ok(()), acted: Ok(row("f", "t", "t", Some("t"))), committed: Err(server("40001")) };
    assert_eq!(
        acting(&pg(Route::Direct), ActionKind::Cancel, Some(answer)).await.outcome,
        ActionOutcome::Acknowledged { via: Via::Standard }
    );
}

#[tokio::test]
async fn an_answer_lost_after_sending_is_never_taken_for_one() {
    let acted = acting(&pg(Route::Direct), ActionKind::Terminate, None).await;
    assert_eq!(acted.outcome, ActionOutcome::OutcomeUnknown);
    assert_eq!(acted.lost, Some(DbError::NoAnswer(ACTION_LIMIT + GRACE)), "the connection is given up");
    let closed = answered(Err(Fail::Connection(DbError::Closed)));
    assert_eq!(acting(&pg(Route::Direct), ActionKind::Cancel, closed).await.outcome, ActionOutcome::OutcomeUnknown);
    let timed_out = answered(Err(server(QUERY_CANCELED)));
    assert_eq!(acting(&pg(Route::Direct), ActionKind::Cancel, timed_out).await.outcome, ActionOutcome::OutcomeUnknown);
}

#[tokio::test]
async fn a_refusal_is_read_from_the_sqlstate() {
    let refused = || answered(Err(server("42501")));
    assert_eq!(
        acting(&pg(Route::Direct), ActionKind::Cancel, refused()).await.outcome,
        ActionOutcome::NotSent(NotSent::Refused(Denied::Grant(&["pg_signal_backend"])))
    );
    let signaller = PgMonitor { signal_backend: true, ..pg(Route::Direct) };
    assert_eq!(
        acting(&signaller, ActionKind::Terminate, refused()).await.outcome,
        ActionOutcome::NotSent(NotSent::Refused(Denied::SuperuserTarget))
    );
    // Another failure before the signal: the transaction's own when it did not begin.
    let begun = DbError::Server("ERROR: begin".into());
    let answer = Pipelined {
        begun: Err(Fail::Server { sqlstate: "22023".into(), error: begun.clone() }),
        acted: Err(server("25P02")),
        committed: Ok(false),
    };
    assert_eq!(
        acting(&pg(Route::Direct), ActionKind::Cancel, Some(answer)).await.outcome,
        ActionOutcome::Failed(begun)
    );
    assert_eq!(
        acting(&pg(Route::Direct), ActionKind::Cancel, answered(Err(server("55P03")))).await.outcome,
        ActionOutcome::Failed(DbError::Server("ERROR: 55P03".into()))
    );
}

#[test]
fn the_route_is_direct_only_when_the_server_names_the_start_ups_process() {
    let pid = |p: &str| vec![vec![Some(p.to_owned())]];
    assert_eq!(route(&pid("4242"), 4242), Route::Direct);
    assert_eq!(route(&pid("4243"), 4242), Route::Pooled);
    assert_eq!(route(&pid("x"), 4242), Route::Unknown);
    assert_eq!(route(&Vec::new(), 4242), Route::Unknown);
    assert_eq!(route(&vec![vec![None]], 4242), Route::Unknown);
}

/// What a cancel and an end send, in order, on every route and provider: the action's
/// transaction (its `BEGIN` with the limits, the one statement that reads and signals the target,
/// the `COMMIT`), and nothing more, refused or not.
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
        for route in [Route::Direct, Route::Pooled, Route::Unknown] {
            let m = PgMonitor { flavour, ..pg(route) };
            for kind in [ActionKind::Cancel, ActionKind::Terminate] {
                let want = [sql::begin(ACTION_LIMIT), sql::action(&m, kind, &target()), "COMMIT".to_owned()];
                let fake = Fake::new(vec![answered(Ok(row("f", "t", "t", Some("t"))))]);
                let acted = act(&fake, &m, &format_of(&m), false, kind, &target()).await;
                assert_eq!(acted.outcome, ActionOutcome::Acknowledged { via: Via::Standard }, "{flavour:?} {route:?}");
                assert_eq!(fake.sent(), want, "{flavour:?} {route:?} {kind:?}");
                let fake = Fake::new(vec![answered(Err(server("42501")))]);
                let acted = act(&fake, &m, &format_of(&m), false, kind, &target()).await;
                assert!(matches!(acted.outcome, ActionOutcome::NotSent(NotSent::Refused(_))), "{:?}", acted.outcome);
                assert_eq!(fake.sent(), want, "{flavour:?} {route:?} {kind:?} refused");
            }
        }
    }
}

/// [`act`] with the engine's monitor rules looked up as `lookup`, checked as an action checks
/// them ([`action::rules_for_with`]), on `fake`.
async fn act_with(
    lookup: Result<&'static dyn MonitorRules, Why>,
    fake: &Fake,
    m: &PgMonitor,
    read_only: bool,
    kind: ActionKind,
) -> Acted {
    let rules = |f: &MonitorFormat| action::rules_for_with(lookup, f);
    act_by(fake, m, &format_of(m), rules, read_only, kind, &target()).await
}

/// Without the engine's monitor rules for the format, or with rules that do not read it,
/// nothing is sent: no transaction at all. A read-only profile is told first.
#[tokio::test]
async fn an_action_without_the_engines_rules_sends_nothing() {
    use datarig_core::engine::{EngineId, Fact};
    let Fact::Known(mysql) = &EngineId::MySql.spec().monitor else { panic!("MySQL has monitor rules") };
    let lookups = [
        (Err(Why::NotBuilt), NotSent::Engine(Why::NotBuilt)),
        (Err(Why::NotChecked), NotSent::Engine(Why::NotChecked)),
        (Ok(*mysql), NotSent::Engine(Why::EnginesDisagree)),
    ];
    for route in [Route::Direct, Route::Pooled] {
        for (lookup, why) in lookups.clone() {
            for kind in [ActionKind::Cancel, ActionKind::Terminate] {
                let fake = Fake::new(Vec::new());
                let acted = act_with(lookup.clone(), &fake, &pg(route), false, kind).await;
                assert_eq!(acted, Acted { outcome: ActionOutcome::NotSent(why.clone()), committed: false, lost: None });
                assert!(fake.sent().is_empty(), "{:?}", fake.sent());
            }
        }
    }
    let fake = Fake::new(Vec::new());
    let acted = act_with(Err(Why::NotBuilt), &fake, &pg(Route::Direct), true, ActionKind::Cancel).await;
    assert_eq!(acted.outcome, ActionOutcome::NotSent(NotSent::ReadOnly));
    assert!(fake.sent().is_empty());
    // The engine's own rules: the action's transaction, as before.
    let Fact::Known(postgres) = &EngineId::Postgres.spec().monitor else { panic!("PostgreSQL has monitor rules") };
    let fake = Fake::new(vec![answered(Ok(row("f", "t", "t", Some("t"))))]);
    let acted = act_with(Ok(*postgres), &fake, &pg(Route::Direct), false, ActionKind::Cancel).await;
    assert_eq!(acted.outcome, ActionOutcome::Acknowledged { via: Via::Standard });
    assert_eq!(fake.sent().len(), 3);
}
