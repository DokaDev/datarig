//! `DbCommand::GroupCount`: the values of one column and the rows that hold each, read as a count
//! is read ([`count`](super::count)) but through the extended protocol, so every value comes in
//! the result format the grid asks for its type and is decoded as a cell of it is
//! ([`formats_of`], [`decode`]).
//!
//! Outside the user's block it runs in a read-only transaction of its own: its first request
//! begins it (with the time and lock wait limits of [`limited_begin`] when it has a limit) and
//! asks the allowlist's question; the statement is then prepared in it (as the unnamed
//! statement, so nothing of it outlives the transaction on a pooler's server connection) and run
//! with the `COMMIT` in the same write. Inside the user's block, and in the transaction of a held
//! portal, it runs under the driver's savepoint without a time limit (the transaction's settings
//! are the user's), released after it and rolled back to when it fails or is cancelled.

use super::*;
use datarig_core::driver::GroupCounts;

/// The values the rows of a group count's statement hold (fetched in `formats`).
fn counts_of(rows: &[Row], formats: &[i16]) -> Result<GroupCounts, DbError> {
    GroupCounts::from_rows(decode(rows, formats))
}

/// Count the rows of each value with `sql` (the app's group count, on the allowlist, checked
/// again here) while no portal is open; also whether it counted in a snapshot fixed for the
/// transaction, as [`count`](super::count) says it.
pub(super) async fn group_count(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    sql: &str,
    limit: Option<Duration>,
) -> Result<(Result<GroupCounts, DbError>, bool), Closed> {
    if !countable(sql) {
        return Ok((Err(DbError::NotSupported), false));
    }
    let check = match check_text(sql) {
        Ok(check) => check,
        Err(why) => return Ok((Err(why), false)),
    };
    // Whether a block is open must be known before a transaction of its own begins.
    if tx.unsure {
        probe(client, link, env, tx).await?;
        if tx.unsure {
            return Ok((Err(DbError::NotRepeatable(risk::repeat::NotRepeatable::TxUnknown)), false));
        }
    }
    if tx.block {
        let sp = env.savepoint;
        let first = tx.take_first_text(env);
        let asked = format!("{first}SAVEPOINT {sp}; {ISOLATION}; {check}");
        let (fixed, answer) = count_checked(link, env, client.simple_query(&asked)).await?;
        let read = match answer {
            Ok(()) => in_block(client, link, env, sql).await?,
            Err(why) => Err(why),
        };
        if let Err(why) = read {
            // Back to before it (an aborted block refuses this too, and stays as it was); the
            // probe tells the block's state either way.
            let _ = link.guard(None, client.batch_execute(&count_undo(env))).await?;
            probe(client, link, env, tx).await?;
            return Ok((Err(why), false));
        }
        return Ok((read, fixed));
    }
    Ok((own(client, link, env, limit, &check, sql).await?, false))
}

/// The read of [`group_count`] in the user's block, under the savepoint its question set: the
/// statement prepared, then run with the savepoint's `RELEASE` in the same write.
async fn in_block(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    sql: &str,
) -> Result<Result<GroupCounts, DbError>, Closed> {
    let prepared = link.guard(Some(env.token), client.prepare_unnamed_wrapped(sql, &[], None)).await?;
    client.forget_typeinfo_statements();
    let stmt = match prepared {
        Ok(stmt) => stmt,
        Err(e) => return Ok(Err(asked_error(&e, env.cancel.asked()))),
    };
    let formats = formats_of(&stmt);
    let release = format!("RELEASE SAVEPOINT {}", env.savepoint);
    let mut txn = client.transaction_in_block_with(Vec::new());
    let read = link.guard(Some(env.token), txn.bind_first_page_then(&stmt, &formats, 0, &release)).await?;
    Ok(match read {
        Ok(page) => counts_of(&page.rows, &formats),
        Err(e) => Err(asked_error(&e, env.cancel.asked())),
    })
}

/// The read of [`group_count`] outside a block, in a read-only transaction of its own (see the
/// module docs): bounded by `limit` as [`limited_read`] bounds a count, and told apart the same
/// way (the server stopping it at its limit is [`DbError::TimedOut`], a cancel `Cancelled`, a
/// lock wait past its limit `LockTimeout`). The transaction is ended whatever happens
/// ([`end_own`]); an answer comes only once the `COMMIT` went through.
async fn own(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    limit: Option<Duration>,
    check: &str,
    sql: &str,
) -> Result<Result<GroupCounts, DbError>, Closed> {
    let begin = match limit {
        Some(limit) => limited_begin(env.each_tx, limit, COUNT_LOCK_WAIT),
        None => std::iter::once(BEGIN_READ_ONLY)
            .chain(env.each_tx.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("; "),
    };
    let first = format!("{begin}; {check}");
    let start = Instant::now();
    let (msgs, end) = link.guard(Some(env.token), streamed(client, &first)).await?;
    let (asked, ran) = (env.cancel.asked(), start.elapsed());
    // Where the allowlist's answer starts: after the limits' answer when there are limits.
    let (set, applied) = match limit {
        Some(_) => {
            let set = limits_end(&msgs);
            (set, set.and_then(|end| applied_limit(&msgs[..end])))
        }
        None => (Some(0), None),
    };
    let failed = |e: &tokio_postgres::Error, asked: bool, ran: Duration, applied: Option<Duration>| {
        if is_cancel(e) { stopped(asked, said(e), ran, applied).unwrap_or_else(|| db_error(e)) } else { db_error(e) }
    };
    let admitted = match (&end, set) {
        (Err(e), _) => Err(failed(e, asked, ran, applied)),
        (Ok(()), None) => Err(DbError::NoResult),
        // A cancel that came after the question found nothing running to stop.
        (Ok(()), Some(_)) if asked => Err(DbError::Cancelled),
        (Ok(()), Some(at)) => check_answer(&msgs[at..]),
    };
    if let Err(why) = admitted {
        if !end.is_err_and(|e| e.is_closed()) {
            end_own(client, link).await?;
        }
        return Ok(Err(why));
    }
    let start = Instant::now();
    let prepared = link.guard(Some(env.token), client.prepare_unnamed_wrapped(sql, &[], None)).await?;
    client.forget_typeinfo_statements();
    let stmt = match prepared {
        Ok(stmt) => stmt,
        Err(e) => {
            let why = failed(&e, env.cancel.asked(), start.elapsed(), applied);
            if !e.is_closed() {
                end_own(client, link).await?;
            }
            return Ok(Err(why));
        }
    };
    let formats = formats_of(&stmt);
    let mut txn = client.transaction_opened();
    let read = link.guard(Some(env.token), txn.bind_first_page_then(&stmt, &formats, 0, "COMMIT")).await?;
    let (asked, ran) = (env.cancel.asked(), start.elapsed());
    match read {
        Ok(page) => {
            drop(txn);
            Ok(counts_of(&page.rows, &formats))
        }
        Err(e) => {
            drop(txn);
            let why = failed(&e, asked, ran, applied);
            if !e.is_closed() {
                end_own(client, link).await?;
            }
            Ok(Err(why))
        }
    }
}

/// [`group_count`] while a portal is open, in the portal's transaction under the driver's
/// savepoint, as [`counted_in`] counts: the portal goes on paging after it, whatever it answers.
pub(super) async fn group_counted_in(
    txn: &Transaction<'_>,
    link: &mut Link,
    env: &Env<'_>,
    sql: &str,
) -> Result<(Result<GroupCounts, DbError>, bool), Closed> {
    if !countable(sql) {
        return Ok((Err(DbError::NotSupported), false));
    }
    let check = match check_text(sql) {
        Ok(check) => check,
        Err(why) => return Ok((Err(why), false)),
    };
    let sp = env.savepoint;
    let asked = format!("SAVEPOINT {sp}; {ISOLATION}; {check}");
    let (fixed, answer) = count_checked(link, env, txn.simple_query(&asked)).await?;
    let read = match answer {
        Ok(()) => match link.guard(Some(env.token), txn.prepare(sql)).await? {
            Ok(stmt) => {
                let formats = formats_of(&stmt);
                match link.guard(Some(env.token), txn.bind_first_page(&stmt, &formats, 0)).await? {
                    Ok(page) => counts_of(&page.rows, &formats),
                    Err(e) => Err(asked_error(&e, env.cancel.asked())),
                }
            }
            Err(e) => Err(asked_error(&e, env.cancel.asked())),
        },
        Err(why) => Err(why),
    };
    match read {
        Ok(counts) => match link.guard(None, txn.batch_execute(&format!("RELEASE SAVEPOINT {sp}"))).await? {
            Ok(()) => Ok((Ok(counts), fixed)),
            Err(e) => {
                let _ = link.guard(None, txn.batch_execute(&count_undo(env))).await?;
                Ok((Err(db_error(&e)), false))
            }
        },
        Err(why) => {
            let _ = link.guard(None, txn.batch_execute(&count_undo(env))).await?;
            Ok((Err(why), false))
        }
    }
}
