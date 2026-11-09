//! `Execute`: the statements of a run, one after another. Each is answered the way the protocol
//! asks: a statement before the last reports its rows (`StepRows`) or its outcome (`Finished`),
//! the last one answers the run (`Page` for rows, `Done` for an outcome).

use crate::session::Ctx;
use crate::values;
use datarig_core::driver::{
    DbError, DbEvent, EXPORT_BATCH_BYTES, ExportBatch, ExportRead, ExportSink, Outcome, TxRule,
};
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, lex_in};
use datarig_core::sql::split::split_in;
use rusqlite::{Connection, ErrorCode};
use std::time::Instant;

/// The statements that change rows: their outcome is how many rows they changed.
const CHANGES_ROWS: [&str; 4] = ["INSERT", "UPDATE", "DELETE", "REPLACE"];

/// What one statement produced.
enum Ran {
    /// A statement that returns rows, with its keyword for the outcome it reports.
    Rows { columns: Vec<datarig_core::driver::ColumnMeta>, rows: Vec<Vec<datarig_core::driver::Cell>>, word: String },
    /// A statement that returns no rows, with its outcome.
    Changed(Outcome),
}

/// A statement that failed, and whether it failed because it was stopped.
struct Failure {
    error: DbError,
    cancelled: bool,
}

pub(crate) fn execute(conn: &Connection, ctx: &Ctx, id: u64, statements: Vec<String>, tx: TxRule) {
    if matches!(tx, TxRule::Manual { .. }) {
        // Manual commit is not offered by this driver: nothing is run.
        return fail(ctx, id, Failure { error: DbError::NotSupported, cancelled: false });
    }
    ctx.start_run();
    let texts: Vec<String> = statements
        .iter()
        .flat_map(|text| split_in(text, Dialect::Sqlite).into_iter().map(|s| s.body(text).to_string()))
        .collect();
    let Some(last) = texts.len().checked_sub(1) else {
        return ctx.send(DbEvent::Done { id, outcome: Outcome::Command(String::new()), elapsed: Default::default() });
    };
    for (index, sql) in texts.iter().enumerate() {
        if texts.len() > 1 {
            if index > 0 && ctx.asked() {
                return fail(ctx, id, Failure { error: DbError::Cancelled, cancelled: true });
            }
            ctx.send(DbEvent::Started { id, index });
        }
        let start = Instant::now();
        let ran = match run(conn, sql) {
            Ok(ran) => ran,
            Err(failure) => return fail(ctx, id, failure),
        };
        let elapsed = start.elapsed();
        match (ran, index == last) {
            (Ran::Rows { columns, rows, .. }, true) => {
                return ctx.send(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed });
            }
            (Ran::Rows { columns, rows, word }, false) => {
                ctx.send(DbEvent::StepRows { id, index, columns: Some(columns), rows, more: false });
                ctx.send(DbEvent::Finished { id, index, outcome: Outcome::Command(word), elapsed });
            }
            (Ran::Changed(outcome), true) => {
                return ctx.send(DbEvent::Done { id, outcome, elapsed });
            }
            (Ran::Changed(outcome), false) => {
                ctx.send(DbEvent::Finished { id, index, outcome, elapsed });
            }
        }
    }
}

fn fail(ctx: &Ctx, id: u64, failure: Failure) {
    ctx.send(DbEvent::Failed { id, error: failure.error, cancelled: failure.cancelled });
}

/// Runs one statement to its end: every row of it is read, and a statement that changes rows
/// reports how many.
fn run(conn: &Connection, sql: &str) -> Result<Ran, Failure> {
    let word = keyword(sql);
    let mut stmt = conn.prepare(sql).map_err(failure)?;
    if stmt.column_count() > 0 {
        let columns = values::columns(&stmt);
        let width = columns.len();
        let mut rows = Vec::new();
        let mut cursor = stmt.query([]).map_err(failure)?;
        while let Some(row) = cursor.next().map_err(failure)? {
            rows.push(values::cells(row, width).map_err(failure)?);
        }
        return Ok(Ran::Rows { columns, rows, word });
    }
    stmt.execute([]).map_err(failure)?;
    let outcome =
        if CHANGES_ROWS.contains(&word.as_str()) { Outcome::Affected(conn.changes()) } else { Outcome::Command(word) };
    Ok(Ran::Changed(outcome))
}

/// An engine error as the statement's failure: an interrupted statement is a cancel.
fn failure(error: rusqlite::Error) -> Failure {
    let cancelled = matches!(&error, rusqlite::Error::SqliteFailure(e, _) if e.code == ErrorCode::OperationInterrupted);
    let error = if cancelled { DbError::Cancelled } else { DbError::Server(error.to_string()) };
    Failure { error, cancelled }
}

/// The first keyword of a statement, upper case (comments before it are skipped).
fn keyword(sql: &str) -> String {
    lex_in(sql, Dialect::Sqlite)
        .into_iter()
        .find(|t| matches!(t.kind, Tok::Keyword | Tok::Ident))
        .map(|t| t.text(sql).to_ascii_uppercase())
        .unwrap_or_default()
}

/// `DbCommand::Export`: every row of one statement, read as the cursor steps and sent to `sink`
/// in batches of at most `batch` rows (and about [`EXPORT_BATCH_BYTES`]). Nothing is held back:
/// the statement is stopped when the sink closes or the run is cancelled.
pub(crate) fn export(conn: &Connection, ctx: &Ctx, id: u64, sql: &str, sink: &ExportSink, batch: usize) {
    ctx.start_run();
    let answer = |result: Result<ExportRead, DbError>, cancelled: bool| {
        ctx.send(DbEvent::Exported { id, result, cancelled });
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(e) => return answer(Err(failure(e).error), false),
    };
    if stmt.column_count() == 0 {
        return answer(Err(DbError::NotSupported), false);
    }
    let columns = values::columns(&stmt);
    let width = columns.len();
    if sink.blocking_send(ExportBatch::Columns(columns)).is_err() {
        return answer(Ok(ExportRead { rows: 0, last: None, complete: false }), false);
    }
    let batch = batch.max(1);
    let mut pending: Vec<Vec<datarig_core::driver::Cell>> = Vec::new();
    let mut pending_bytes = 0usize;
    let (mut rows, mut last) = (0u64, None);
    let mut cursor = match stmt.query([]) {
        Ok(cursor) => cursor,
        Err(e) => return answer(Err(failure(e).error), false),
    };
    loop {
        let step = match cursor.next() {
            Ok(Some(row)) => values::cells(row, width),
            Ok(None) => break,
            Err(e) => {
                let f = failure(e);
                return answer(Err(f.error), f.cancelled);
            }
        };
        let cells = match step {
            Ok(cells) => cells,
            Err(e) => return answer(Err(failure(e).error), false),
        };
        pending_bytes += cells.iter().map(|c| c.as_ref().map_or(0, String::len)).sum::<usize>();
        rows += 1;
        last = Some(cells.clone());
        pending.push(cells);
        if pending.len() >= batch || pending_bytes >= EXPORT_BATCH_BYTES {
            let full = std::mem::take(&mut pending);
            pending_bytes = 0;
            if sink.blocking_send(ExportBatch::Rows(full)).is_err() {
                return answer(Ok(ExportRead { rows, last, complete: false }), false);
            }
        }
    }
    if !pending.is_empty() && sink.blocking_send(ExportBatch::Rows(pending)).is_err() {
        return answer(Ok(ExportRead { rows, last, complete: false }), false);
    }
    answer(Ok(ExportRead { rows, last, complete: true }), false)
}
