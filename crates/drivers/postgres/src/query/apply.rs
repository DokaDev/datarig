//! `DbCommand::ApplyEdits`: a result's changes, as the statements `edit` wrote for them, in one
//! transaction. Each step is one round trip (the unnamed statement, every column in text), its
//! count read from the server's command tag and compared with the one it must give before the
//! next goes out; the transaction's start and its end are one more each.
//!
//! Inside a block (the user's, or one opened for manual commit) the steps run under the driver's
//! savepoint [`SAVEPOINT`]: released when every step stood, rolled back to otherwise, so the
//! block is as it was and not aborted. Outside a block they run in a transaction of their own
//! that starts as the session's do ([`Env::begin`]: the search path and the lock wait limit
//! behind a pooler go in it, nothing is left on the server connection), committed after the
//! last step, or left open for manual commit; rolled back when a step fails.

use super::{Closed, Env, Link, Tx, columns_of, decode, idle_limit, probe};
use crate::connect::{db_error, is_cancel};
use datarig_core::driver::{AppliedStep, ApplyError, ApplyFailure, ApplyTx, DbError, DbEvent};
use datarig_core::edit::{Expect, Step};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_postgres::Client;

/// The savepoint an Apply runs under inside a block.
const SAVEPOINT: &str = "datarig_apply";

/// Back to before [`SAVEPOINT`]: the block as it was before the Apply, not aborted.
const UNDO: &str = "ROLLBACK TO SAVEPOINT datarig_apply; RELEASE SAVEPOINT datarig_apply";

/// Where the steps run.
#[derive(Clone, Copy)]
enum Mode {
    /// Under [`SAVEPOINT`] in the open block.
    Savepoint,
    /// In a transaction of their own, committed after them.
    Own,
    /// In a transaction opened for manual commit (its idle limit), left open after them.
    Keep(Option<Duration>),
}

fn fail(step: Option<usize>, error: DbError) -> ApplyFailure {
    ApplyFailure { step, why: ApplyError::Db(error) }
}

/// A step's failure as the answer says it (a cancel is `Cancelled`).
fn step_error(e: &tokio_postgres::Error) -> DbError {
    if is_cancel(e) { DbError::Cancelled } else { db_error(e) }
}

/// The count a step must give.
fn wanted(expect: Expect) -> u64 {
    match expect {
        Expect::Rows(n) | Expect::Affected(n) => n,
    }
}

/// Apply `steps` for request `id` and answer with exactly one `Applied`, then the state. A
/// session the UI closed meanwhile answers nothing; a cancel that could not be sent comes back
/// for the caller to answer.
pub(super) async fn apply(
    client: &Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    id: u64,
    steps: &[Step],
    rule: ApplyTx,
) -> Result<(), Closed> {
    env.cancel.store(false, Ordering::SeqCst);
    env.failed.store(false, Ordering::SeqCst);
    let answer = |result| {
        let _ = env.events.send(DbEvent::Applied { id, result });
    };
    if env.read_only {
        answer(Err(fail(None, DbError::ReadWriteRefused)));
        return Ok(());
    }
    if tx.unsure {
        probe(client, link, env, tx).await?;
    }
    if tx.block && tx.aborted {
        answer(Err(fail(None, DbError::NotSupported)));
        return Ok(());
    }
    if steps.is_empty() {
        answer(Ok(Vec::new()));
        return Ok(());
    }
    if client.is_closed() {
        tx.unsure = true;
        answer(Err(fail(None, DbError::Closed)));
        return Ok(());
    }
    let mode = match rule {
        _ if tx.block => Mode::Savepoint,
        ApplyTx::Commit => Mode::Own,
        ApplyTx::Keep { idle_timeout } => Mode::Keep(idle_timeout),
    };
    let start = match mode {
        Mode::Savepoint => {
            let mut first = tx.take_first(env);
            first.push(format!("SAVEPOINT {SAVEPOINT}"));
            first
        }
        Mode::Own => env.begin(),
        Mode::Keep(idle) => {
            let mut begin = env.begin();
            begin.extend(idle.map(idle_limit));
            begin
        }
    };
    if let Err(e) = link.guard(None, client.batch_execute(&start.join("; "))).await? {
        // A block opened by it is aborted, a savepoint not set: back to before it.
        let undo = if matches!(mode, Mode::Savepoint) { "ROLLBACK TO SAVEPOINT datarig_apply" } else { "ROLLBACK" };
        let _ = link.guard(None, client.batch_execute(undo)).await?;
        probe(client, link, env, tx).await?;
        answer(Err(fail(None, db_error(&e))));
        return Ok(());
    }
    let mut done = Vec::with_capacity(steps.len());
    let mut failure = None;
    for (i, step) in steps.iter().enumerate() {
        if i > 0 && env.cancel.load(Ordering::SeqCst) {
            failure = Some(fail(Some(i), DbError::Cancelled));
            break;
        }
        // Every row it matches (0: no limit), so a check locks them all.
        let ran = link.guard(Some(env.token), client.execute_pipelined_wrapped(&step.sql, 0, &[], None)).await?;
        let ran = match ran {
            Ok(ran) => ran,
            Err(e) => {
                failure = Some(fail(Some(i), step_error(&e)));
                break;
            }
        };
        let Some(count) = ran.rows_affected else {
            failure = Some(fail(Some(i), DbError::NoResult));
            break;
        };
        if count != wanted(step.expect) {
            failure = Some(ApplyFailure { step: Some(i), why: ApplyError::Count { got: count } });
            break;
        }
        let returned = match (step.expect, ran.statement) {
            (Expect::Affected(_), Some(stmt)) => {
                let text = vec![0; stmt.columns().len()];
                Some((columns_of(stmt.columns()), decode(&ran.rows, &text)))
            }
            _ => None,
        };
        done.push(AppliedStep { count, returned, insert_id: None });
    }
    if let Some(failure) = failure {
        let undo = if matches!(mode, Mode::Savepoint) { UNDO } else { "ROLLBACK" };
        if link.guard(None, client.batch_execute(undo)).await?.is_err() {
            probe(client, link, env, tx).await?;
        }
        answer(Err(failure));
        return Ok(());
    }
    match mode {
        Mode::Savepoint => {
            let released = link.guard(None, client.batch_execute(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))).await?;
            if let Err(e) = released {
                if link.guard(None, client.batch_execute(UNDO)).await?.is_err() {
                    probe(client, link, env, tx).await?;
                }
                answer(Err(fail(None, db_error(&e))));
                return Ok(());
            }
            answer(Ok(done));
        }
        Mode::Own => match link.guard(None, client.command_tag("COMMIT")).await? {
            Ok(tag) if tag == "COMMIT" => answer(Ok(done)),
            // The block was aborted after all: the server rolled it back.
            Ok(_) => answer(Err(fail(None, DbError::NoResult))),
            // The connection ended (a server error that ends the session is a `FATAL` one):
            // whether the server committed is not known.
            Err(e) if e.as_db_error().is_none_or(|d| matches!(d.severity(), "FATAL" | "PANIC")) => {
                tx.unsure = true;
                answer(Err(fail(None, DbError::CommitUnknown)));
            }
            // Refused (a deferred constraint): the server rolled back and ended the block.
            Err(e) => answer(Err(fail(None, db_error(&e)))),
        },
        Mode::Keep(idle) => {
            (tx.block, tx.aborted, tx.first_pending) = (true, false, false);
            (tx.idle, tx.idle_due) = (idle, false);
            tx.opening = Some((id, 0));
            answer(Ok(done));
            // Reported as a block opened for manual commit.
            env.report(tx, true);
        }
    }
    Ok(())
}
