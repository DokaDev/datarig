//! `DbCommand::ApplyEdits`: a result's changes, as the statements `edit` wrote for them, in one
//! transaction on the tab's own session (so a temporary table that shadows a table is the one
//! its rows came from). Each step is one request; a check's count is the rows it returned, a
//! write's the rows it matched (the session runs with `CLIENT_FOUND_ROWS`, so a value set to
//! what it was counts). The count is compared with the one it must give before the next goes
//! out.
//!
//! Inside the user's transaction (or one opened for manual commit) the steps run under the
//! driver's savepoint [`SAVEPOINT`]: released when every step stood, rolled back to otherwise,
//! so the transaction keeps what it held before (unless the server rolls all of it back, a
//! deadlock: said with `TxClosedBy`). Outside one they run in a transaction of their own,
//! committed after the last step, or left open for manual commit; rolled back when a step fails.

use super::{COMMIT, Closed, Env, Link, ROLLBACK, START, State, Stop, after, drain, failure, idle, rest};
use crate::session::my_error;
use datarig_core::driver::{AppliedStep, ApplyError, ApplyFailure, ApplyTx, DbError, DbEvent};
use datarig_core::edit::{Expect, Step};
use mysql_async::Conn;
use mysql_async::prelude::Queryable;
use std::sync::atomic::Ordering;

/// The savepoint an Apply runs under inside a transaction.
const SAVEPOINT: &str = "datarig_apply";

/// Where the steps run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Under [`SAVEPOINT`] in the open transaction.
    Savepoint,
    /// In a transaction of their own, committed after them.
    Own,
    /// In a transaction opened for manual commit, left open after them.
    Keep,
}

fn fail(step: Option<usize>, error: DbError) -> ApplyFailure {
    ApplyFailure { step, why: ApplyError::Db(error) }
}

/// The count a step must give.
fn wanted(expect: Expect) -> u64 {
    match expect {
        Expect::Rows(n) | Expect::Affected(n) => n,
    }
}

/// One step: the rows it returned (when it returns rows) or matched, and the `AUTO_INCREMENT`
/// value an insert took.
async fn run_step(conn: &mut Conn, sql: &str) -> Result<(u64, Option<u64>), mysql_async::Error> {
    let mut qr = conn.query_iter(sql).await?;
    let returns = qr.columns().is_some_and(|c| !c.is_empty());
    let rows = drain(&mut qr).await?;
    let (affected, insert_id) = (qr.affected_rows(), qr.last_insert_id());
    rest(&mut qr).await?;
    Ok((if returns { rows } else { affected }, insert_id.filter(|n| *n > 0)))
}

/// One statement of the driver's own around the steps.
async fn own(conn: &mut Conn, link: &mut Link, sql: &str) -> Result<Result<(), mysql_async::Error>, Closed> {
    link.guard(None, conn.query_drop(sql)).await
}

/// Apply `steps` for request `id` and answer with exactly one `Applied`, then the state. A
/// session the UI closed meanwhile answers nothing; a lost one answers before it is said lost.
pub(super) async fn apply(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    id: u64,
    steps: &[Step],
    rule: ApplyTx,
) -> Result<(), Stop> {
    link.serving(env.events, id);
    s.request = id;
    env.asked.store(false, Ordering::SeqCst);
    env.failed.store(false, Ordering::SeqCst);
    env.killer.clear();
    let answer = |result| {
        let _ = env.events.send(DbEvent::Applied { id, result });
    };
    if env.read_only {
        answer(Err(fail(None, DbError::ReadWriteRefused)));
        return Ok(());
    }
    // The statements need what only MySQL 8.0.17 on has; the app does not send them otherwise.
    if env.server.edit().is_none() {
        answer(Err(fail(None, DbError::NotSupported)));
        return Ok(());
    }
    if steps.is_empty() {
        answer(Ok(Vec::new()));
        return Ok(());
    }
    let mode = match rule {
        _ if s.tx.open => Mode::Savepoint,
        ApplyTx::Commit => Mode::Own,
        ApplyTx::Keep { .. } => Mode::Keep,
    };
    // A check's rows come whatever the session's limit is: one that could cut them is reset.
    let most = steps.iter().filter_map(|st| matches!(st.expect, Expect::Rows(_)).then_some(wanted(st.expect))).max();
    if let (Some(most), Some(limit)) = (most, s.limit.current)
        && limit <= most
    {
        if let Err(e) = own(conn, link, "SET SESSION sql_select_limit = DEFAULT").await? {
            return lost_or(e, answer);
        }
        s.tracked.update(conn);
        s.tracked.select_limit = None;
        s.limit.current = None;
    }
    let start = if mode == Mode::Savepoint { format!("SAVEPOINT {SAVEPOINT}") } else { START.to_string() };
    if let Err(e) = own(conn, link, &start).await? {
        return lost_or(e, answer);
    }
    let mut done = Vec::with_capacity(steps.len());
    let mut failed = None;
    for (i, step) in steps.iter().enumerate() {
        // A cancel asked for, or a connection that runs nothing more (a kill not answered).
        if env.asked.load(Ordering::SeqCst) || link.tainted() {
            failed = Some((fail(Some(i), DbError::Cancelled), None));
            break;
        }
        env.busy.store(true, Ordering::SeqCst);
        let ran = match link.guard(Some(env.killer), run_step(conn, &step.sql)).await {
            Ok(ran) => ran,
            // A kill that did not take cut the connection: the step ends cancelled all the same,
            // and the server rolls back what is open as the connection ends.
            Err(Closed::CancelNotTaken(why)) => {
                answer(Err(fail(Some(i), DbError::Cancelled)));
                return Err(Stop::Lost(DbError::CancelNotTaken(why)));
            }
            Err(Closed::Ui) => return Err(Stop::Closed),
        };
        idle(env, link).await;
        match ran {
            Ok((count, _)) if count != wanted(step.expect) => {
                failed = Some((ApplyFailure { step: Some(i), why: ApplyError::Count { got: count } }, None));
                break;
            }
            Ok((count, insert_id)) => {
                let insert_id = insert_id.filter(|_| matches!(step.expect, Expect::Affected(_)));
                done.push(AppliedStep { count, returned: None, insert_id });
            }
            Err(e) => {
                failed = Some((fail(Some(i), failure(&e).0), Some(e)));
                break;
            }
        }
    }
    // A cancel asked for once the last step ran: nothing is kept.
    if failed.is_none() && env.asked.load(Ordering::SeqCst) {
        failed = Some((fail(None, DbError::Cancelled), None));
    }
    // A connection that runs nothing more (a kill not answered) is cut at its next request:
    // nothing more reaches the server, which rolls back what is open as the connection ends.
    if link.tainted() {
        let failure = failed.map_or(fail(None, DbError::Cancelled), |(f, _)| f);
        answer(Err(failure));
        return Ok(());
    }
    if let Some((failure, error)) = failed {
        if let Some(e) = error.filter(|e| e.is_fatal()) {
            answer(Err(failure));
            return Err(Stop::Lost(my_error(&e)));
        }
        let undo =
            if mode == Mode::Savepoint { format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}") } else { ROLLBACK.to_string() };
        let mut undone = own(conn, link, &undo).await?.is_ok();
        if mode == Mode::Savepoint && undone {
            undone = own(conn, link, &format!("RELEASE SAVEPOINT {SAVEPOINT}")).await?.is_ok();
        }
        answer(Err(failure));
        // The server may have rolled the whole transaction back (a deadlock), or kept one the
        // undo did not end: the state as it says.
        if (mode == Mode::Savepoint || !undone)
            && let Some(error) = after(conn, link, env, s, "", false, true, false).await?
        {
            return Err(Stop::Lost(error));
        }
        return Ok(());
    }
    match mode {
        Mode::Savepoint => {
            if let Err(e) = own(conn, link, &format!("RELEASE SAVEPOINT {SAVEPOINT}")).await? {
                if e.is_fatal() {
                    return lost_or(e, answer);
                }
                let _ = own(conn, link, &format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}")).await?;
                answer(Err(fail(None, my_error(&e))));
                if let Some(error) = after(conn, link, env, s, "", false, true, false).await? {
                    return Err(Stop::Lost(error));
                }
                return Ok(());
            }
            answer(Ok(done));
        }
        Mode::Own => match own(conn, link, COMMIT).await? {
            Ok(()) => answer(Ok(done)),
            Err(e) if e.is_fatal() => {
                answer(Err(fail(None, DbError::CommitUnknown)));
                return Err(Stop::Lost(my_error(&e)));
            }
            // Refused: the server says what it left, and it is rolled back.
            Err(e) => {
                let undone = own(conn, link, ROLLBACK).await?.is_ok();
                answer(Err(fail(None, my_error(&e))));
                if !undone && let Some(error) = after(conn, link, env, s, "", false, true, false).await? {
                    return Err(Stop::Lost(error));
                }
            }
        },
        Mode::Keep => {
            answer(Ok(done));
            // Reported as a transaction opened for manual commit.
            s.tx.opening = Some((id, 0));
            if let Some(error) = after(conn, link, env, s, START, false, true, true).await? {
                return Err(Stop::Lost(error));
            }
        }
    }
    Ok(())
}

/// Answer the failure of a statement of the driver's own around the steps (nothing of them was
/// kept); a connection that failed for good is lost after it.
fn lost_or(e: mysql_async::Error, answer: impl Fn(Result<Vec<AppliedStep>, ApplyFailure>)) -> Result<(), Stop> {
    answer(Err(fail(None, my_error(&e))));
    if e.is_fatal() { Err(Stop::Lost(my_error(&e))) } else { Ok(()) }
}
