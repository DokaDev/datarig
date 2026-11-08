//! The *monitor* connection (`SessionRole::Monitor`): what the server says about its sessions
//! and its load, for the Monitor tab. The statements are the driver's ([`crate::meta::monitor`],
//! next to the catalog's); the reading of their rows into the model is the core's
//! (`datarig_core::monitor::decode`). This runs them.
//!
//! Every read is one transaction of its own, `READ ONLY`, with a short lock wait and a
//! statement limit ([`crate::meta::monitor::begin`]), opened, read and committed in one round trip: the
//! `BEGIN`, each statement (simple queries, so every value comes back as text and nothing is
//! prepared that a pooler could lose) and the `COMMIT` go out together. A failed statement
//! leaves the transaction aborted, which the `COMMIT` rolls back, so nothing stays open on the
//! connection; everything the transaction set ends with it, so nothing stays on a server
//! connection a pooler hands to another client.
//!
//! An action on another session ([`act`]) is a transaction of its own too, `READ ONLY` like
//! the reads (the signal functions write nothing): its one statement reads the target and
//! signals it only if it is still what the row showed, and what that statement answered is kept
//! whatever becomes of the `COMMIT`. An action is never sent twice: an answer lost on the way is
//! reported as such, for a later read to tell.
//!
//! The statements read only the activity and statistics views and settings: no user's table is
//! touched; the only locks are the brief `ACCESS SHARE` locks of the catalogs those views read,
//! waited for at most a second. The connection runs nothing before its probe (no read-only
//! check, context or limits: [`crate::connect`] skips them for a monitor), so the
//! transactions counted in [`Own`] are all it ever committed.

use crate::connect::db_error;
use crate::link::{Link, Next};
use crate::meta::monitor as sql;
use datarig_core::driver::{Cell, DbCommand, DbError, DbEvent};
use datarig_core::monitor::action::{
    self, ActionKind, ActionOutcome, ActionTarget, FollowUp, NativeError, NotSent, Stale, Via,
};
use datarig_core::monitor::decode::{self, Rows};
use datarig_core::monitor::format::PG_OLDEST;
use datarig_core::monitor::{
    FullText, MonitorFormat, Own, PgMonitor, Replication, Route, SessionsWanted, Snapshot, TextAsk, Want,
};
use futures::future::join_all;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_postgres::{Client, SimpleQueryMessage};

/// How long the probe's statement may run.
const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// How long the read of a statement's whole text may run.
const TEXT_LIMIT: Duration = Duration::from_secs(5);
/// How long an action's statement may run.
const ACTION_LIMIT: Duration = Duration::from_secs(5);

/// How much longer than its statements' limit a read may take before the connection is given
/// up on (the server stops each statement at the limit; past this, it or the network does not
/// answer).
const GRACE: Duration = Duration::from_secs(3);

/// `read_only`: the profile's own (the monitor connection is read-only whatever the profile
/// says, so this is the only place that tells an action may not be sent).
pub(crate) async fn monitor_loop(client: Client, mut link: Link, events: UnboundedSender<DbEvent>, read_only: bool) {
    // Transactions this connection committed: a tick's counters are read inside the next one,
    // so they count every earlier one and not the current.
    let mut own = Own::default();
    // Closed by the app while a read waits: the connection is closed at once, not left to the
    // read's own limit.
    let Ok(probed) = link.guard(None, probe(&client, &mut own)).await else {
        link.close();
        return;
    };
    let format = match probed {
        Ok(format) => Some(format),
        Err(Failure::Lost(error)) => {
            link.close();
            let _ = events.send(DbEvent::Lost { error });
            return;
        }
        Err(Failure::Failed(error)) => {
            let _ = events.send(DbEvent::Monitor(Err(error)));
            None
        }
    };
    if let Some(pg) = &format {
        let _ = events.send(DbEvent::Monitor(Ok(MonitorFormat::Postgres(pg.clone()))));
    }
    loop {
        let cmd = match link.next().await {
            Next::Command(c) => c,
            Next::Closed => return,
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        let Some(pg) = &format else {
            // Without a format there is nothing to read with.
            if events.send(cmd.not_supported()).is_err() {
                return;
            }
            continue;
        };
        let (answer, lost) = match cmd {
            DbCommand::MonitorTick { request, want } => {
                let Ok(result) = link.guard(None, tick(&client, pg, &want, &mut own)).await else {
                    link.close();
                    return;
                };
                match result {
                    Ok(snapshot) => (DbEvent::MonitorSnapshot { request, result: Ok(snapshot) }, None),
                    Err(Failure::Failed(error)) => (DbEvent::MonitorSnapshot { request, result: Err(error) }, None),
                    Err(Failure::Lost(error)) => {
                        (DbEvent::MonitorSnapshot { request, result: Err(error.clone()) }, Some(error))
                    }
                }
            }
            DbCommand::MonitorQueryText { request, ask } => {
                let Ok(result) = link.guard(None, query_text(&client, &ask, &mut own)).await else {
                    link.close();
                    return;
                };
                match result {
                    Ok(text) => (DbEvent::MonitorQueryText { request, result: Ok(text) }, None),
                    Err(Failure::Failed(error)) => (DbEvent::MonitorQueryText { request, result: Err(error) }, None),
                    Err(Failure::Lost(error)) => {
                        (DbEvent::MonitorQueryText { request, result: Err(error.clone()) }, Some(error))
                    }
                }
            }
            DbCommand::MonitorReplication { request, limit } => {
                let Ok(result) = link.guard(None, replication(&client, pg, limit, &mut own)).await else {
                    link.close();
                    return;
                };
                match result {
                    Ok(read) => (DbEvent::MonitorReplication { request, result: Ok(read) }, None),
                    Err(Failure::Failed(error)) => (DbEvent::MonitorReplication { request, result: Err(error) }, None),
                    Err(Failure::Lost(error)) => {
                        (DbEvent::MonitorReplication { request, result: Err(error.clone()) }, Some(error))
                    }
                }
            }
            DbCommand::MonitorAction { request, kind, target } => {
                let acted = act(&client, pg, read_only, kind, &target);
                let Ok(acted) = link.guard(None, acted).await else {
                    link.close();
                    return;
                };
                if acted.committed {
                    own.transactions += 1;
                }
                (DbEvent::MonitorActionDone { request, outcome: acted.outcome }, acted.lost)
            }
            DbCommand::MonitorFollowUp { request, target } => {
                let Ok(result) = link.guard(None, follow_up(&client, pg, &target, &mut own)).await else {
                    link.close();
                    return;
                };
                match result {
                    Ok(follow) => (DbEvent::MonitorFollowUp { request, result: Ok(follow) }, None),
                    Err(Failure::Failed(error)) => (DbEvent::MonitorFollowUp { request, result: Err(error) }, None),
                    Err(Failure::Lost(error)) => {
                        (DbEvent::MonitorFollowUp { request, result: Err(error.clone()) }, Some(error))
                    }
                }
            }
            // Anything else is another session's work.
            other => (other.not_supported(), None),
        };
        if let Some(error) = lost {
            link.close();
            let _ = events.send(answer);
            let _ = events.send(DbEvent::Lost { error });
            return;
        }
        if events.send(answer).is_err() {
            return;
        }
    }
}

/// Why a read gave no answer.
enum Failure {
    /// The server refused or failed it; the connection is fine.
    Failed(DbError),
    /// It took past its limit and grace: the connection cannot be trusted any more.
    Lost(DbError),
}

/// What the server is and the account may do, and how the connection reaches the server: the
/// server process that answers is the one whose id came in the start-up's `BackendKeyData`
/// only on a direct connection (a pooler sends an id of its own). Read in the probe's own
/// transaction, so it costs no round trip more.
async fn probe(client: &Client, own: &mut Own) -> Result<PgMonitor, Failure> {
    let rows = read(client, &[sql::probe(), sql::backend_pid()], PROBE_LIMIT, own).await?;
    let mut pg = decode::pg::probe(&rows[0]).map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))?;
    pg.route = route(&rows[1], client.backend_key_pid());
    Ok(pg)
}

/// How the connection reaches the server, from the server's answer for its own process id
/// (`rows`) and the id the start-up named (`key_pid`): direct only when both are the same.
fn route(rows: &Rows, key_pid: i32) -> Route {
    match rows.as_slice() {
        [row] => match row.first().and_then(|c| c.as_deref()).map(str::parse::<i32>) {
            Some(Ok(pid)) if pid == key_pid => Route::Direct,
            Some(Ok(_)) => Route::Pooled,
            _ => Route::Unknown,
        },
        _ => Route::Unknown,
    }
}

/// The whole text of the statement `ask` names, in a read of its own (counted like a tick's).
async fn query_text(client: &Client, ask: &TextAsk, own: &mut Own) -> Result<FullText, Failure> {
    let rows = read(client, &[sql::query_text(ask.session.id)], TEXT_LIMIT, own).await?;
    decode::pg::query_text(ask, &rows[0]).map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))
}

async fn tick(client: &Client, pg: &PgMonitor, want: &Want, own: &mut Own) -> Result<Snapshot, Failure> {
    let before = *own;
    let rows = read(client, &sql::tick(pg, want), want.limit, own).await?;
    decode::pg::tick(want, &rows, before).map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))
}

/// What the server says about its replication, in a read-only transaction of its own.
async fn replication(client: &Client, pg: &PgMonitor, limit: Duration, own: &mut Own) -> Result<Replication, Failure> {
    let rows = read(client, &sql::replication::read(pg), limit, own).await?;
    decode::pg::replication::read(&rows).map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))
}

/// The session `target` names as a read now shows it, with the tick's columns.
async fn follow_up(client: &Client, pg: &PgMonitor, target: &ActionTarget, own: &mut Own) -> Result<FollowUp, Failure> {
    let rows = read(client, &[sql::session(pg, target.key.id)], ACTION_LIMIT, own).await?;
    let (list, _) = decode::pg::sessions(&rows[0], SessionsWanted { idle: true })
        .map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))?;
    let row = list.rows.into_iter().find(|r| r.key.id == target.key.id);
    Ok(FollowUp { row, interrupted: None })
}

/// What an action became, and what it did to the connection.
#[derive(Debug, PartialEq)]
struct Acted {
    outcome: ActionOutcome,
    /// Its transaction committed (one more of the connection's own).
    committed: bool,
    /// The connection is past trusting: the answer did not come in time.
    lost: Option<DbError>,
}

/// How a statement of an action's transaction failed.
#[derive(Clone, Debug, PartialEq)]
enum Fail {
    /// The server refused or failed it: its SQLSTATE, read before it becomes a [`DbError`].
    Server { sqlstate: String, error: DbError },
    /// No answer from the server (the connection ended): whether it ran is not known.
    Connection(DbError),
}

impl Fail {
    fn of(e: &tokio_postgres::Error) -> Fail {
        match e.as_db_error() {
            Some(db) => Fail::Server { sqlstate: db.code().code().to_owned(), error: db_error(e) },
            None => Fail::Connection(db_error(e)),
        }
    }
}

/// The answers of an action's transaction, sent in one go: the `BEGIN` with its settings, the
/// action's statement, the `COMMIT`.
#[derive(Clone, Debug, PartialEq)]
struct Pipelined {
    begun: Result<(), Fail>,
    acted: Result<Rows, Fail>,
    /// The `COMMIT`'s answer: whether it committed (a transaction a failed statement aborted
    /// answers `ROLLBACK`).
    committed: Result<bool, Fail>,
}

/// What sends an action's transaction: the connection, or a test's stand-in.
trait Pipeline {
    /// `begin`, `statement` and `COMMIT` in one round trip; `None` when no answer came within
    /// `deadline`.
    async fn send(&self, begin: &str, statement: &str, deadline: Duration) -> Option<Pipelined>;
}

impl Pipeline for Client {
    async fn send(&self, begin: &str, statement: &str, deadline: Duration) -> Option<Pipelined> {
        let all =
            async { tokio::join!(self.batch_execute(begin), self.simple_query(statement), self.command_tag("COMMIT")) };
        let (begun, acted, committed) = tokio::time::timeout(deadline, all).await.ok()?;
        Some(Pipelined {
            begun: begun.map_err(|e| Fail::of(&e)),
            acted: acted.map(text_rows).map_err(|e| Fail::of(&e)),
            committed: committed.map(|tag| tag == "COMMIT").map_err(|e| Fail::of(&e)),
        })
    }
}

/// The SQLSTATE of a statement cancelled (here: past its `statement_timeout`).
const QUERY_CANCELED: &str = "57014";

/// Send `kind` on `target` in a transaction of its own; on a `read_only` profile nothing is
/// sent at all.
async fn act(p: &impl Pipeline, pg: &PgMonitor, read_only: bool, kind: ActionKind, target: &ActionTarget) -> Acted {
    let done = |outcome| Acted { outcome, committed: false, lost: None };
    if read_only {
        return done(ActionOutcome::NotSent(NotSent::ReadOnly));
    }
    let deadline = ACTION_LIMIT + GRACE;
    let Some(answers) = p.send(&sql::begin(ACTION_LIMIT), &sql::action(pg, kind, target), deadline).await else {
        return Acted {
            outcome: ActionOutcome::OutcomeUnknown,
            committed: false,
            lost: Some(DbError::NoAnswer(deadline)),
        };
    };
    // Only a transaction the server committed is one of the connection's own commits.
    let committed = answers.committed == Ok(true);
    // The statement's answer stands whatever became of the `COMMIT`.
    let outcome = match answers.acted {
        Ok(rows) => outcome(&rows),
        Err(Fail::Server { sqlstate, error }) => {
            let format = MonitorFormat::Postgres(pg.clone());
            match action::denied(&format, target, Via::Standard, NativeError::Pg { sqlstate: &sqlstate }) {
                Some(why) => ActionOutcome::NotSent(NotSent::Refused(why)),
                // The limit may have stopped it after the signal went.
                None if sqlstate == QUERY_CANCELED => ActionOutcome::OutcomeUnknown,
                None => match answers.begun {
                    Err(Fail::Server { error, .. } | Fail::Connection(error)) => ActionOutcome::Failed(error),
                    Ok(()) => ActionOutcome::Failed(error),
                },
            }
        }
        Err(Fail::Connection(_)) => ActionOutcome::OutcomeUnknown,
    };
    Acted { outcome, committed, lost: None }
}

/// The action statement's answer ([`sql::action`]) as what became of the action.
fn outcome(rows: &Rows) -> ActionOutcome {
    let [row] = rows.as_slice() else {
        return match rows.len() {
            0 => ActionOutcome::NotSent(NotSent::Stale(Stale::SessionGone)),
            // A pid names one session: rows that say otherwise were not read as meant.
            _ => ActionOutcome::OutcomeUnknown,
        };
    };
    let flag = |i: usize| row.get(i).and_then(|c| c.as_deref());
    if flag(0) == Some("t") {
        ActionOutcome::NotSent(NotSent::OwnConnection)
    } else if flag(1) != Some("t") {
        ActionOutcome::NotSent(NotSent::Stale(Stale::SessionReplaced))
    } else if flag(2) != Some("t") {
        // Activity not tracked (or not readable): what it runs cannot be told apart.
        ActionOutcome::NotSent(NotSent::Stale(Stale::CannotTell))
    } else if flag(3) != Some("t") {
        ActionOutcome::NotSent(NotSent::Stale(Stale::StatementChanged))
    } else {
        match flag(4) {
            Some("t") => ActionOutcome::Acknowledged { via: Via::Standard },
            Some("f") => ActionOutcome::NotDelivered,
            // Every condition held, so the function was called: its answer is not what it gives.
            _ => ActionOutcome::OutcomeUnknown,
        }
    }
}

/// Run `statements` in one read-only transaction, in one round trip, and count it in `own` when
/// it committed.
async fn read(client: &Client, statements: &[String], limit: Duration, own: &mut Own) -> Result<Vec<Rows>, Failure> {
    let begin = sql::begin(limit);
    let all = async {
        tokio::join!(
            client.batch_execute(&begin),
            join_all(statements.iter().map(|s| client.simple_query(s))),
            client.batch_execute("COMMIT"),
        )
    };
    let Ok((begun, answers, committed)) = tokio::time::timeout(limit + GRACE, all).await else {
        return Err(Failure::Lost(DbError::NoAnswer(limit + GRACE)));
    };
    begun.map_err(|e| Failure::Failed(db_error(&e)))?;
    let mut rows = Vec::with_capacity(answers.len());
    for answer in answers {
        rows.push(text_rows(answer.map_err(|e| Failure::Failed(db_error(&e)))?));
    }
    committed.map_err(|e| Failure::Failed(db_error(&e)))?;
    own.transactions += 1;
    Ok(rows)
}

/// A simple query's rows as text cells.
fn text_rows(messages: Vec<SimpleQueryMessage>) -> Rows {
    messages
        .into_iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(row) => {
                Some((0..row.len()).map(|i| row.get(i).map(str::to_owned)).collect::<Vec<Cell>>())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
