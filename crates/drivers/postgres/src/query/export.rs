//! `DbCommand::Export`: every row of one statement into the export's bounded channel, a
//! portal page at a time, the next page asked for only once the channel took the one before
//! (so a slow file holds the statement back instead of rows piling up in memory).
//!
//! Outside the user's block the statement runs in a `BEGIN READ ONLY` transaction of its own
//! (with the session's search path when it is set per transaction, and a keyset chunk's
//! `lock_timeout`), committed after the last page: one statement, one snapshot, nothing
//! written whatever it calls. Inside the user's block it runs under the driver's savepoint
//! ([`Env::savepoint`]), released after it and rolled back to when it fails or is cancelled,
//! so the block is never aborted by it; an aborted block refuses it unasked.

use super::{
    BEGIN_READ_ONLY, Closed, Env, Link, State, check_text, checked, columns_of, copies_with_client, count_error,
    count_undo, decode, finish, formats_of, probe,
};
use crate::connect::{db_error, is_cancel};
use datarig_core::driver::{DbError, ExportBatch, ExportGuard, ExportRead, ExportSink};
use datarig_core::export::keyset::LOCK_WAIT_SECS;
use datarig_core::sql::risk;
use std::sync::atomic::Ordering;
use tokio_postgres::{Client, FirstPage, Statement, Transaction};

/// An export's answer: what it read, or why not and whether that was a cancel.
pub(super) type Answer = (Result<ExportRead, DbError>, bool);

/// Run export `sql` into `sink`, `batch` rows a page ([`DbCommand::Export`]).
///
/// [`DbCommand::Export`]: datarig_core::driver::DbCommand::Export
#[allow(clippy::too_many_arguments)]
pub(super) async fn export(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State<'_>,
    sql: &str,
    guard: ExportGuard,
    sink: ExportSink,
    batch: usize,
) -> Result<Answer, Closed> {
    if s.tx.unsure {
        probe(client, link, env, s.tx).await?;
    }
    // A cancel asked for before this belonged to what ran before.
    env.cancel.store(false, Ordering::SeqCst);
    if s.tx.block && s.tx.aborted {
        return Ok((Err(DbError::NotSupported), false));
    }
    if copies_with_client(sql) {
        return Ok((Err(DbError::CopyStdio), false));
    }
    let r = risk::classify(sql);
    if env.read_only && r.read_write {
        return Ok((Err(DbError::ReadWriteRefused), false));
    }
    // An export reads: nothing that writes or locks rows runs for one.
    if r.class != risk::Class::Read || r.writes {
        return Ok((Err(DbError::NotSupported), false));
    }
    let check = match guard {
        ExportGuard::Repeatable => match check_text(sql) {
            Ok(check) => Some(check),
            Err(why) => return Ok((Err(why), false)),
        },
        ExportGuard::Confirmed | ExportGuard::Keyset => None,
    };
    let max = i32::try_from(batch.max(1)).unwrap_or(i32::MAX);
    let sp = env.savepoint;
    if s.tx.block {
        // The savepoint (made read-only before it when that is due), and the allowlist's
        // question, in one request.
        let first = s.tx.take_first_text(env);
        let asked = match &check {
            Some(c) => format!("{first}SAVEPOINT {sp}; {c}"),
            None => format!("{first}SAVEPOINT {sp}"),
        };
        let (_, answer) = checked(link, env, client.simple_query(&asked)).await?;
        let stmt = match answer {
            Ok(()) => prepared(client, link, env, s, sql, &[]).await?,
            Err(why) => Err(why),
        };
        let stmt = match stmt {
            Ok(stmt) => stmt,
            Err(why) => {
                // An aborted block refuses this too, and stays as it was.
                let _ = link.guard(None, client.batch_execute(&count_undo(env))).await?;
                probe(client, link, env, s.tx).await?;
                let cancelled = why == DbError::Cancelled;
                return Ok((Err(why), cancelled));
            }
        };
        let txn = client.transaction_in_block();
        let out = stream(&txn, link, env, &stmt, &sink, max).await?;
        drop(sink);
        let answer = match out {
            Ok(read) => match link.guard(None, txn.batch_execute(&format!("RELEASE SAVEPOINT {sp}"))).await? {
                Ok(()) => (Ok(read), false),
                Err(e) => {
                    // Not released (a cancel that came late): back to before it, so the block
                    // is not left aborted.
                    let _ = link.guard(None, txn.batch_execute(&count_undo(env))).await?;
                    (Err(count_error(&e)), is_cancel(&e))
                }
            },
            Err((error, cancelled)) => {
                let _ = link.guard(None, txn.batch_execute(&count_undo(env))).await?;
                (Err(error), cancelled)
            }
        };
        drop(txn);
        probe(client, link, env, s.tx).await?;
        return Ok(answer);
    }
    if let Some(c) = &check {
        let path = env.path_text();
        if let (_, Err(why)) = checked(link, env, client.simple_query(&format!("{path}{c}"))).await? {
            let cancelled = why == DbError::Cancelled;
            return Ok((Err(why), cancelled));
        }
    }
    // Its own read-only transaction, opened in the request that parses the statement.
    let mut begin = vec![BEGIN_READ_ONLY.to_string()];
    begin.extend(env.path.map(str::to_string));
    if guard == ExportGuard::Keyset {
        begin.push(format!("SET LOCAL lock_timeout = '{LOCK_WAIT_SECS}s'"));
    }
    let stmt = match prepared(client, link, env, s, sql, &begin).await? {
        Ok(stmt) => stmt,
        Err(why) => {
            // The failure left the transaction it was parsed in open and aborted.
            let _ = link.guard(None, client.batch_execute("ROLLBACK")).await?;
            let cancelled = why == DbError::Cancelled;
            return Ok((Err(why), cancelled));
        }
    };
    let txn = client.transaction_opened();
    let out = stream(&txn, link, env, &stmt, &sink, max).await?;
    drop(sink);
    Ok(match out {
        Ok(read) => match finish(txn, true, link).await? {
            Ok(()) => (Ok(read), false),
            Err(e) => (Err(db_error(&e)), false),
        },
        Err((error, cancelled)) => {
            let _ = finish(txn, false, link).await?;
            (Err(error), cancelled)
        }
    })
}

/// `sql` parsed as the unnamed statement after `before` (a transaction's start), refused when it
/// returns no rows (an export reads; nothing else runs).
async fn prepared(
    client: &Client,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State<'_>,
    sql: &str,
    before: &[String],
) -> Result<Result<Statement, DbError>, Closed> {
    let stmt = link.guard(Some(env.token), client.prepare_unnamed_wrapped(sql, before, None)).await?;
    if s.prepared.off {
        // A type lookup's statements go with the transaction too.
        client.forget_typeinfo_statements();
    }
    Ok(match stmt {
        Ok(stmt) if stmt.columns().is_empty() => Err(DbError::NotSupported),
        Ok(stmt) => Ok(stmt),
        Err(e) => Err(count_error(&e)),
    })
}

/// Send `batch` to the export's channel, waiting for room: `false` when nobody reads it any
/// more (the file failed).
async fn send(link: &mut Link, sink: &ExportSink, batch: ExportBatch) -> Result<bool, Closed> {
    Ok(link.guard(None, sink.send(batch)).await?.is_ok())
}

/// Bind `stmt` and send its rows, `max` a page, each page asked for after the one before was
/// taken. A cancel between two pages stops it before the next.
async fn stream(
    txn: &Transaction<'_>,
    link: &mut Link,
    env: &Env<'_>,
    stmt: &Statement,
    sink: &ExportSink,
    max: i32,
) -> Result<Result<ExportRead, (DbError, bool)>, Closed> {
    let formats = formats_of(stmt);
    let FirstPage { portal, columns, rows, complete, .. } =
        match link.guard(Some(env.token), txn.bind_first_page(stmt, &formats, max)).await? {
            Ok(first) => first,
            Err(e) => return Ok(Err((count_error(&e), is_cancel(&e)))),
        };
    let mut read = ExportRead { rows: 0, last: None, complete: true };
    if !send(link, sink, ExportBatch::Columns(columns_of(&columns))).await? {
        read.complete = false;
        return Ok(Ok(read));
    }
    let (mut rows, mut ended) = (rows, complete);
    loop {
        if !rows.is_empty() {
            let page = decode(&rows, &formats);
            read.rows += page.len() as u64;
            read.last = page.last().cloned();
            if !send(link, sink, ExportBatch::Rows(page)).await? {
                read.complete = false;
                break;
            }
        }
        if ended {
            break;
        }
        // Cancelled while the page was being written: nothing runs on the server now, so the
        // cancel stops it here.
        if env.cancel.load(Ordering::SeqCst) {
            return Ok(Err((DbError::Cancelled, true)));
        }
        match link.guard(Some(env.token), txn.query_portal(&portal, max)).await? {
            Ok(more) => {
                ended = (more.len() as i32) < max;
                rows = more;
            }
            Err(e) => return Ok(Err((count_error(&e), is_cancel(&e)))),
        }
    }
    drop(portal);
    Ok(Ok(read))
}
