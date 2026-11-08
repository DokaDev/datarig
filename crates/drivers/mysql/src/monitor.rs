//! The *monitor* connection (`SessionRole::Monitor`): what the server says about its sessions
//! and its load, for the Monitor tab. The statements are the driver's ([`crate::meta::monitor`],
//! next to the catalog's); the reading of their rows into the model is the core's
//! (`datarig_core::monitor::decode`). This runs them.
//!
//! Every statement is a read on its own in autocommit, on a session set up read-only, with a
//! metadata lock wait of a second, `SELECT`s under the tick's limit, no `sql_select_limit` and
//! times in the server's system time zone (`session::init_sql`). The statements read only `performance_schema`, `information_schema`
//! and the server's status and variables: no user's table is touched. A refusal for lack of a
//! privilege is an answer of the probe (what the account may read), never a failure of it.
//!
//! An action on another session ([`act`]) reads the target right before it (its session and
//! statement as the tick tells them, compared in the core) and sends `KILL QUERY` or `KILL`
//! only when it is still what the row showed; on a managed server that refuses it, the
//! provider's procedure, after the target is read again. A few milliseconds lie between the
//! read and the statement: the server offers no statement that checks and kills in one. An
//! action is never sent twice: an answer lost on the way is reported as such.
//!
//! The server counts every statement a client sends in `Questions` as it starts it, so the
//! statements this connection sent are counted here one by one ([`Own::statements`]) and taken
//! out of the rates exactly.

use crate::link::{Link, Next};
use crate::meta::monitor as sql;
use crate::session::{my_error, quit};
use datarig_core::driver::{Cell, DbCommand, DbError, DbEvent};
use datarig_core::monitor::action::{
    self, ActionKind, ActionOutcome, ActionTarget, FollowUp, NativeError, NotSent, Stale, Via,
};
use datarig_core::monitor::decode::{self, Answer, Rows};
use datarig_core::monitor::format::MYSQL_OLDEST;
use datarig_core::monitor::{
    FullText, MonitorFormat, MySqlMonitor, Own, Replication, SessionsWanted, Snapshot, StatementStamp, TextAsk, Want,
};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Row, Value};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// How long the probe may take.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// How long the read of a statement's whole text may run when no tick has set a limit yet.
const TEXT_LIMIT: Duration = Duration::from_secs(5);

/// How much longer than its statements' limit a tick may take before the connection is given up
/// on (`max_execution_time` does not stop a `SHOW`; past this, the server or the network does not
/// answer).
const GRACE: Duration = Duration::from_secs(3);

/// How long an action (its reads and the statement that acts) may take before the connection
/// is given up on.
const ACTION_DEADLINE: Duration = Duration::from_secs(10);

/// The connection and what it sent.
struct Monitor {
    conn: Conn,
    /// Statements sent since the session opened (the set-up statements before are counted by
    /// the server too, but before any sample, so no rate spans them).
    sent: u64,
    /// The `max_execution_time` set on the session, once a tick set one.
    limit: Option<Duration>,
}

/// `read_only`: the profile's own (the monitor session is read-only whatever the profile says,
/// so this is the only place that tells an action may not be sent).
pub(crate) async fn monitor_loop(conn: Conn, mut link: Link, events: UnboundedSender<DbEvent>, read_only: bool) {
    let mut m = Monitor { conn, sent: 0, limit: None };
    let probed = match link.guard(None, tokio::time::timeout(PROBE_LIMIT, m.probe())).await {
        Ok(Ok(probed)) => probed,
        Ok(Err(_)) => {
            link.cut();
            let _ = events.send(DbEvent::Lost { error: DbError::NoAnswer(PROBE_LIMIT) });
            return;
        }
        // Closed by the app: the connection was cut.
        Err(_) => return,
    };
    let format = match probed {
        Ok(my) => {
            let _ = events.send(DbEvent::Monitor(Ok(MonitorFormat::MySql(my.clone()))));
            Some(my)
        }
        Err(error) => {
            let _ = events.send(DbEvent::Monitor(Err(error)));
            None
        }
    };
    loop {
        let cmd = match link.next().await {
            Next::Command(c) => c,
            Next::Closed => {
                quit(m.conn).await;
                return;
            }
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        let Some(my) = &format else {
            // Without a format there is nothing to read with.
            if events.send(cmd.not_supported()).is_err() {
                return;
            }
            continue;
        };
        let (answer, lost) = match cmd {
            DbCommand::MonitorTick { request, want } => {
                let deadline = want.limit + GRACE;
                match link.guard(None, tokio::time::timeout(deadline, m.tick(my, &want))).await {
                    Ok(Ok(result)) => (DbEvent::MonitorSnapshot { request, result }, None),
                    Ok(Err(_)) => {
                        let error = DbError::NoAnswer(deadline);
                        (DbEvent::MonitorSnapshot { request, result: Err(error.clone()) }, Some(error))
                    }
                    Err(_) => return,
                }
            }
            DbCommand::MonitorQueryText { request, ask } => {
                let deadline = m.limit.unwrap_or(TEXT_LIMIT) + GRACE;
                match link.guard(None, tokio::time::timeout(deadline, m.query_text(&ask))).await {
                    Ok(Ok(result)) => (DbEvent::MonitorQueryText { request, result }, None),
                    Ok(Err(_)) => {
                        let error = DbError::NoAnswer(deadline);
                        (DbEvent::MonitorQueryText { request, result: Err(error.clone()) }, Some(error))
                    }
                    Err(_) => return,
                }
            }
            DbCommand::MonitorReplication { request, limit } => {
                let deadline = limit + GRACE;
                match link.guard(None, tokio::time::timeout(deadline, m.replication(my, limit))).await {
                    Ok(Ok(result)) => (DbEvent::MonitorReplication { request, result }, None),
                    Ok(Err(_)) => {
                        let error = DbError::NoAnswer(deadline);
                        (DbEvent::MonitorReplication { request, result: Err(error.clone()) }, Some(error))
                    }
                    Err(_) => return,
                }
            }
            DbCommand::MonitorAction { request, kind, target } => {
                let Ok((outcome, lost)) =
                    link.guard(None, within(&mut m, ACTION_DEADLINE, my, read_only, kind, &target)).await
                else {
                    return;
                };
                (DbEvent::MonitorActionDone { request, outcome }, lost)
            }
            DbCommand::MonitorFollowUp { request, target } => {
                let read = tokio::time::timeout(ACTION_DEADLINE, follow_up(&mut m, my, &target));
                match link.guard(None, read).await {
                    Ok(Ok(result)) => (DbEvent::MonitorFollowUp { request, result }, None),
                    Ok(Err(_)) => {
                        let error = DbError::NoAnswer(ACTION_DEADLINE);
                        (DbEvent::MonitorFollowUp { request, result: Err(error.clone()) }, Some(error))
                    }
                    Err(_) => return,
                }
            }
            // Anything else is another session's work.
            other => (other.not_supported(), None),
        };
        if let Some(error) = lost {
            link.cut();
            let _ = events.send(answer);
            let _ = events.send(DbEvent::Lost { error });
            return;
        }
        if events.send(answer).is_err() {
            return;
        }
        if m.conn.is_disconnected() {
            let _ = events.send(DbEvent::Lost { error: DbError::Closed });
            return;
        }
    }
}

impl Monitor {
    /// The rows of `sql`, as text cells.
    async fn rows(&mut self, sql: &str) -> Result<Rows, mysql_async::Error> {
        self.sent += 1;
        let rows: Vec<Row> = self.conn.query(sql).await?;
        Ok(rows.into_iter().map(|row| row.unwrap().into_iter().map(text).collect()).collect())
    }

    /// What the server offers, the account may read and the provider is.
    async fn probe(&mut self) -> Result<MySqlMonitor, DbError> {
        let mut answers = Vec::new();
        for statement in sql::probe() {
            answers.push(match self.rows(&statement).await {
                Ok(rows) => Answer::Rows(rows),
                Err(e) => Answer::Error { code: server_code(&e), message: my_error(&e).raw().into_owned() },
            });
        }
        decode::mysql::probe(&answers).map_err(|e| e.into_db(MYSQL_OLDEST))
    }

    async fn tick(&mut self, my: &MySqlMonitor, want: &Want) -> Result<Snapshot, DbError> {
        if self.limit != Some(want.limit) {
            self.rows(&sql::limit(want.limit)).await.map_err(|e| my_error(&e))?;
            self.limit = Some(want.limit);
        }
        let mut rows = Vec::new();
        let mut own = Own::default();
        let mut locks = None;
        let sessions_at = want.sessions.map(|_| 2);
        for (i, statement) in sql::tick(my, want).iter().enumerate() {
            rows.push(self.rows(statement).await.map_err(|e| my_error(&e))?);
            // The counters are the second statement: the server counted every statement sent
            // until then, that one with them.
            if i == 1 {
                own.statements = self.sent;
            }
            // The lock reads right after the session list (they are told apart by it).
            let last = i + 1 == 2 + usize::from(want.sessions.is_some());
            if want.locks && last {
                locks = Some(self.locks(my, sessions_at.map(|at| &rows[at])).await?);
            }
        }
        decode::mysql::tick(my, want, &rows, locks.as_deref(), own).map_err(|e| e.into_db(MYSQL_OLDEST))
    }

    /// The lock reads of a tick, after its session list (`sessions`, `None`: not read): sent
    /// only when a session may wait for a lock; else each answers as no wait. A refusal for
    /// lack of a privilege is an answer (what the account may read), never a failure.
    async fn locks(&mut self, my: &MySqlMonitor, sessions: Option<&Rows>) -> Result<Vec<Answer>, DbError> {
        let statements = sql::locks(my);
        let may = match sessions {
            Some(rows) => decode::mysql::may_wait(rows, my).map_err(|e| e.into_db(MYSQL_OLDEST))?,
            None => true,
        };
        if !may {
            return Ok(statements.iter().map(|_| Answer::Rows(Vec::new())).collect());
        }
        let mut answers = Vec::with_capacity(statements.len());
        for statement in &statements {
            answers.push(match self.rows(statement).await {
                Ok(rows) => Answer::Rows(rows),
                Err(e) if server_code(&e).is_some_and(decode::mysql::refused) => {
                    Answer::Error { code: server_code(&e), message: my_error(&e).raw().into_owned() }
                }
                Err(e) => return Err(my_error(&e)),
            });
        }
        Ok(answers)
    }

    /// What the server says about its replication: each statement answered or refused on its
    /// own (a refusal for lack of a privilege is said as such); any other failure fails the read.
    async fn replication(&mut self, my: &MySqlMonitor, limit: Duration) -> Result<Replication, DbError> {
        if self.limit != Some(limit) {
            self.rows(&sql::limit(limit)).await.map_err(|e| my_error(&e))?;
            self.limit = Some(limit);
        }
        let mut answers = Vec::new();
        for statement in sql::replication::read(my) {
            let answer = match self.named_rows(&statement.sql, statement.columns).await {
                Ok(rows) => Answer::Rows(rows),
                Err(e @ mysql_async::Error::Server(_)) => {
                    Answer::Error { code: server_code(&e), message: my_error(&e).raw().into_owned() }
                }
                Err(e) => return Err(my_error(&e)),
            };
            answers.push((statement.read, answer));
        }
        decode::mysql::replication::read(my, &answers).map_err(|e| e.into_db(MYSQL_OLDEST))
    }

    /// The rows of `sql` as text cells; with `columns`, only those, by name, in that order (a
    /// column missing is the statement's own mistake: an error, never a NULL).
    async fn named_rows(&mut self, sql: &str, columns: Option<&[&str]>) -> Result<Rows, mysql_async::Error> {
        let Some(columns) = columns else {
            return self.rows(sql).await;
        };
        self.sent += 1;
        let result = self.conn.query_iter(sql).await?;
        let names: Vec<String> = result.columns_ref().iter().map(|c| c.name_str().into_owned()).collect();
        let mut at = Vec::with_capacity(columns.len());
        for name in columns {
            match names.iter().position(|n| n == name) {
                Some(i) => at.push(i),
                None => {
                    // Read the rest of the answer so the connection stays usable.
                    result.drop_result().await?;
                    return Err(mysql_async::Error::Other(format!("no column {name} in the answer").into()));
                }
            }
        }
        let rows: Vec<Row> = result.collect_and_drop().await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let values = row.unwrap();
                at.iter().map(|&i| text(values[i].clone())).collect()
            })
            .collect())
    }

    /// The whole text of the statement `ask` names, in a statement of its own (under the
    /// session's statement limit, set first when no tick has set one).
    async fn query_text(&mut self, ask: &TextAsk) -> Result<FullText, DbError> {
        if self.limit.is_none() {
            self.rows(&sql::limit(TEXT_LIMIT)).await.map_err(|e| my_error(&e))?;
            self.limit = Some(TEXT_LIMIT);
        }
        let rows = self.rows(&sql::query_text(ask.session.id)).await.map_err(|e| my_error(&e))?;
        decode::mysql::query_text(ask, &rows).map_err(|e| e.into_db(MYSQL_OLDEST))
    }
}

/// How a statement of an action failed.
#[derive(Clone, Debug, PartialEq)]
enum Fail {
    /// The server refused or failed it: its error code, read before it becomes a [`DbError`].
    Server { code: u16, error: DbError },
    /// No answer from the server (the connection failed): whether it ran is not known.
    Connection(DbError),
}

impl Fail {
    fn of(e: &mysql_async::Error) -> Fail {
        match e {
            mysql_async::Error::Server(s) => Fail::Server { code: s.code, error: my_error(e) },
            _ => Fail::Connection(my_error(e)),
        }
    }

    fn into_error(self) -> DbError {
        match self {
            Fail::Server { error, .. } | Fail::Connection(error) => error,
        }
    }
}

/// What runs an action's statements: the monitor's connection, or a test's stand-in.
trait Exec {
    async fn run(&mut self, sql: &str) -> Result<Rows, Fail>;
}

impl Exec for Monitor {
    async fn run(&mut self, sql: &str) -> Result<Rows, Fail> {
        self.rows(sql).await.map_err(|e| Fail::of(&e))
    }
}

/// `ER_NO_SUCH_THREAD`: no session of that id (it ended between the read and the statement).
const NO_SUCH_THREAD: u16 = 1094;

/// `ER_QUERY_INTERRUPTED`: the statement was stopped by a `KILL QUERY` (or `KILL`).
const QUERY_INTERRUPTED: &str = "1317";

/// [`act`] given at most `deadline`; past it, the connection is given up on (the error), and
/// the outcome is unknown once the statement that acts may have gone out.
async fn within(
    x: &mut impl Exec,
    deadline: Duration,
    my: &MySqlMonitor,
    read_only: bool,
    kind: ActionKind,
    target: &ActionTarget,
) -> (ActionOutcome, Option<DbError>) {
    let mut sending = false;
    match tokio::time::timeout(deadline, act(x, my, read_only, kind, target, &mut sending)).await {
        Ok(outcome) => (outcome, None),
        Err(_) => {
            let error = DbError::NoAnswer(deadline);
            let outcome = if sending { ActionOutcome::OutcomeUnknown } else { ActionOutcome::Failed(error.clone()) };
            (outcome, Some(error))
        }
    }
}

/// Send `kind` on `target` if a read right before finds it unchanged; on a `read_only` profile
/// nothing is sent at all. `sending` is set before the statement that acts goes out.
async fn act(
    x: &mut impl Exec,
    my: &MySqlMonitor,
    read_only: bool,
    kind: ActionKind,
    target: &ActionTarget,
    sending: &mut bool,
) -> ActionOutcome {
    if read_only {
        return ActionOutcome::NotSent(NotSent::ReadOnly);
    }
    // No session has an id that is not a positive integer.
    let Ok(id) = u64::try_from(target.key.id) else {
        return ActionOutcome::NotSent(NotSent::Stale(Stale::SessionGone));
    };
    if let Err(outcome) = recheck(x, my, kind, target, id).await {
        return outcome;
    }
    let format = MonitorFormat::MySql(my.clone());
    let route = action::route(&format);
    *sending = true;
    let failed = match x.run(&sql::kill(kind, id)).await {
        Ok(_) => return ActionOutcome::Acknowledged { via: Via::Standard },
        Err(fail) => fail,
    };
    let fallback = match (&failed, route.fallback) {
        (Fail::Server { code, .. }, Some(fallback)) if fallback.on.contains(code) => fallback,
        _ => return refused(&format, target, Via::Standard, failed),
    };
    // The standard statement was refused: nothing reached the target. The provider's procedure
    // goes only to the same target, read again.
    *sending = false;
    if let Err(outcome) = recheck(x, my, kind, target, id).await {
        return outcome;
    }
    let procedure = fallback.procedure(kind);
    *sending = true;
    match x.run(&sql::call(procedure, id)).await {
        Ok(_) => ActionOutcome::Acknowledged { via: Via::Procedure(procedure) },
        Err(fail) => refused(&format, target, Via::Procedure(procedure), fail),
    }
}

/// Read the target again: `Err` with the outcome when nothing may be sent (or the read
/// failed, so nothing was).
async fn recheck(
    x: &mut impl Exec,
    my: &MySqlMonitor,
    kind: ActionKind,
    target: &ActionTarget,
    id: u64,
) -> Result<(), ActionOutcome> {
    let rows = x.run(&sql::guard(my, id)).await.map_err(|f| ActionOutcome::Failed(f.into_error()))?;
    let (list, _) = decode::mysql::sessions(&[rows], my, SessionsWanted { idle: true })
        .map_err(|e| ActionOutcome::Failed(e.into_db(MYSQL_OLDEST)))?;
    let now = list.rows.iter().find(|r| r.key.id == target.key.id);
    match action::recheck(kind, target, now) {
        Some(why) => Err(ActionOutcome::NotSent(why)),
        None => Ok(()),
    }
}

/// What a failed statement that acts (sent `via`) means.
fn refused(format: &MonitorFormat, target: &ActionTarget, via: Via, fail: Fail) -> ActionOutcome {
    match fail {
        Fail::Server { code: NO_SUCH_THREAD, .. } => ActionOutcome::NotSent(NotSent::Stale(Stale::SessionGone)),
        Fail::Server { code, error } => match action::denied(format, target, via, NativeError::MySql { errno: code }) {
            Some(why) => ActionOutcome::NotSent(NotSent::Refused(why)),
            None => ActionOutcome::Failed(error),
        },
        // It may have reached the server.
        Fail::Connection(_) => ActionOutcome::OutcomeUnknown,
    }
}

/// The session `target` names as a read now shows it, with the tick's columns, and whether its
/// statement event ended interrupted (when the target names one).
async fn follow_up(x: &mut impl Exec, my: &MySqlMonitor, target: &ActionTarget) -> Result<FollowUp, DbError> {
    let Ok(id) = u64::try_from(target.key.id) else {
        return Ok(FollowUp { row: None, interrupted: None });
    };
    let rows = x.run(&sql::session(my, id)).await.map_err(Fail::into_error)?;
    let (list, _) =
        decode::mysql::sessions(&[rows], my, SessionsWanted { idle: true }).map_err(|e| e.into_db(MYSQL_OLDEST))?;
    let row = list.rows.into_iter().find(|r| r.key.id == target.key.id);
    let interrupted = match target.stamp {
        Some(StatementStamp::MySqlEvent { thread_id, event_id }) => {
            match x.run(&sql::statement_errno(thread_id, event_id)).await {
                Ok(rows) => match rows.as_slice() {
                    [row] => row.first().and_then(|c| c.as_deref()).map(|errno| errno == QUERY_INTERRUPTED),
                    _ => None,
                },
                // Not readable (a privilege, the consumer off): not known.
                Err(_) => None,
            }
        }
        _ => None,
    };
    Ok(FollowUp { row, interrupted })
}

/// A value of the text protocol as the text the server sent.
fn text(value: Value) -> Cell {
    match value {
        Value::NULL => None,
        Value::Bytes(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        other => Some(other.as_sql(false)),
    }
}

/// The server's error code of a failed statement, `None` for a failure of another kind.
fn server_code(e: &mysql_async::Error) -> Option<u32> {
    match e {
        mysql_async::Error::Server(s) => Some(u32::from(s.code)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
