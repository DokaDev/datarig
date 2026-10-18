//! `DbCommand::Export`: every row of one statement into the export's bounded channel, read off
//! the result as the channel takes them (a slow file holds the statement back instead of rows
//! piling up in memory). The statement runs without the session's paging limit
//! (`sql_select_limit` is the user's own, if any): an export is never cut at a page.
//!
//! It reads only (a statement that writes or locks rows is refused unsent) and runs where the
//! session is: inside the user's transaction it reads there, in the transaction's snapshot
//! (a failed read does not end a MySQL transaction). A cancel is the session's `KILL QUERY`;
//! when nobody reads the channel any more (the file failed) the result is stopped the same way
//! and the answer says it was not read to its end.
//!
//! While the file is slow the server waits to send the rest, at most its `net_write_timeout`
//! (60 s by default): past that it drops the connection, the export fails and the session is
//! lost, as a connection that broke.

use super::{
    Env, Link, State, Stop, after, asked_server, failure, idle, limit_applies, read, rest, runs_server_code,
    stop_result,
};
use crate::session::my_error;
use crate::values::column_meta;
use datarig_core::driver::{ColumnMeta, DbError, ExportBatch, ExportGuard, ExportRead, ExportSink};
use datarig_core::sql::risk::Class;
use datarig_core::sql::risk::mysql as risk;
use mysql_async::Conn;
use mysql_async::prelude::Queryable;
use std::sync::atomic::Ordering;

/// An export's answer: what it read, or why not and whether that was a cancel.
pub(super) type Answer = (Result<ExportRead, DbError>, bool);

/// Run export `sql` into `sink`, `batch` rows at a time ([`DbCommand::Export`]).
///
/// [`DbCommand::Export`]: datarig_core::driver::DbCommand::Export
#[allow(clippy::too_many_arguments)]
pub(super) async fn export(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    sql: &str,
    guard: ExportGuard,
    sink: ExportSink,
    batch: usize,
) -> Result<Answer, Stop> {
    // A cancel asked for before this belonged to what ran before.
    env.asked.store(false, Ordering::SeqCst);
    env.killer.clear();
    let r = risk::classify(sql, s.mode);
    if env.read_only && (r.read_write || r.read_only().is_err()) {
        return Ok((Err(DbError::ReadWriteRefused), false));
    }
    if r.class != Class::Read || r.writes {
        return Ok((Err(DbError::NotSupported), false));
    }
    if guard == ExportGuard::Repeatable {
        if let Err(why) = risk::repeatable(sql, s.mode) {
            return Ok((Err(DbError::NotRepeatable(why)), false));
        }
        if let Err(why) = asked_server(conn, link, env, s, sql).await? {
            let cancelled = why == DbError::Cancelled;
            return Ok((Err(why), cancelled));
        }
    }
    // No limit of the app's: the user's own, if they set one.
    let wanted = s.limit.user;
    if limit_applies(sql, s.mode) && wanted != s.limit.current {
        let set = format!("SET SESSION sql_select_limit = {}", wanted.map_or("DEFAULT".to_string(), |n| n.to_string()));
        if let Err(e) = link.guard(None, conn.query_drop(set)).await? {
            return Ok((Err(my_error(&e)), false));
        }
        s.tracked.update(conn);
        s.tracked.select_limit = None;
        s.limit.current = wanted;
    }
    s.routine = runs_server_code(sql, s.mode, r.runs_code);
    let was_open = s.tx.open;
    env.busy.store(true, Ordering::SeqCst);
    let sent = link.guard(Some(env.killer), conn.query_iter(sql)).await?;
    let mut qr = match sent {
        Ok(qr) => qr,
        Err(e) => {
            idle(env, link).await;
            return failed(conn, link, env, s, &e, was_open).await;
        }
    };
    let Some(columns) = qr.columns().filter(|c| !c.is_empty()) else {
        // No rows to read: what it did is read to its end (nothing for a read).
        let done = link.guard(Some(env.killer), rest(&mut qr)).await?;
        drop(qr);
        idle(env, link).await;
        if let Err(e) = done {
            return failed(conn, link, env, s, &e, was_open).await;
        }
        if let Some(error) = after(conn, link, env, s, sql, r.implicit_commit, was_open, true).await? {
            return Err(Stop::Lost(error));
        }
        return Ok((Err(DbError::NotSupported), false));
    };
    let metas: Vec<ColumnMeta> = columns.iter().map(column_meta).collect();
    let mut read_rows = ExportRead { rows: 0, last: None, complete: true };
    let mut stopped = link.guard(None, sink.send(ExportBatch::Columns(metas))).await?.is_err();
    let mut cancelled = false;
    let batch = batch.max(1);
    while !stopped {
        if env.asked.load(Ordering::SeqCst) {
            cancelled = true;
            stopped = true;
            break;
        }
        match link.guard(Some(env.killer), read(&mut qr, &columns, batch)).await? {
            Ok((rows, ended)) => {
                if !rows.is_empty() {
                    read_rows.rows += rows.len() as u64;
                    read_rows.last = rows.last().cloned();
                    if link.guard(None, sink.send(ExportBatch::Rows(rows))).await?.is_err() {
                        stopped = true;
                        break;
                    }
                }
                if ended {
                    break;
                }
            }
            Err(e) => {
                drop(qr);
                idle(env, link).await;
                return failed(conn, link, env, s, &e, was_open).await;
            }
        }
    }
    drop(sink);
    if stopped {
        // Not read to its end: stopped on the server, the rest read and dropped.
        stop_result(&mut qr, link, env).await?;
        drop(qr);
        idle(env, link).await;
        if let Some(error) = after(conn, link, env, s, sql, false, true, false).await? {
            return Err(Stop::Lost(error));
        }
        if cancelled {
            return Ok((Err(DbError::Cancelled), true));
        }
        read_rows.complete = false;
        return Ok((Ok(read_rows), false));
    }
    // A procedure's further result sets (none for a read).
    let done = link.guard(Some(env.killer), rest(&mut qr)).await?;
    drop(qr);
    idle(env, link).await;
    if let Err(e) = done {
        return failed(conn, link, env, s, &e, was_open).await;
    }
    if let Some(error) = after(conn, link, env, s, sql, r.implicit_commit, was_open, true).await? {
        return Err(Stop::Lost(error));
    }
    Ok((Ok(read_rows), false))
}

/// The statement failed (`e`): its answer, after the session asked the server what it left. A
/// broken connection ends the session.
async fn failed(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    e: &mysql_async::Error,
    was_open: bool,
) -> Result<Answer, Stop> {
    if e.is_fatal() {
        return Err(Stop::Lost(my_error(e)));
    }
    let (error, cancelled) = failure(e);
    if let Some(lost) = after(conn, link, env, s, "", false, was_open || cancelled, false).await? {
        return Err(Stop::Lost(lost));
    }
    Ok((Err(error), cancelled))
}
