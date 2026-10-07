//! The *query* connection: the user's statements, one per request in MySQL's text protocol
//! (the client never asks for multi-statements: the app splits scripts itself, `DELIMITER`
//! included), their rows page by page, and what each one did.
//!
//! **Paging without holding anything** (`PagingMode::NoHold`, outside the user's transaction):
//! MySQL has no server-side cursor a client can keep open without keeping its statement running,
//! and a running statement holds the metadata locks it took (a concurrent `ALTER TABLE` waits for
//! them, and every later statement on that table waits behind the `ALTER`). So the last
//! statement of a run runs with `sql_select_limit` at a page and one row more: the server stops
//! the result there, the statement ends, and with it every lock it took; the extra row says that
//! more follow (`DbEvent::Released` comes before such a page). The limit never changes what a
//! write does (it applies to a `SELECT`'s own result only, not to subqueries, `INSERT … SELECT` or
//! stored routines), and a `LIMIT` of the user's own wins. A result the limit does not stop
//! (`SHOW`, a procedure's, a larger `LIMIT`) is read to its end and only its first page kept. Past
//! the first page the app can only run the statement again (`Resume`: the limit then covers the
//! rows it has too, which are read and dropped). The limit is the session's, set only when the
//! next statement that may return rows needs another one (no round trip in the common case);
//! one the user sets (`SET sql_select_limit`) is kept for the statements it applies to.
//!
//! **Held results** (`PagingMode::Hold`, and every result inside the user's transaction): no
//! limit; the result is read page by page as the app asks (`FetchMore`), and its statement runs
//! (and holds its locks) until it is read to its end. The app reads such a result to its end at
//! once (`Capabilities::reads_held_to_end`), into its own store. One that is not read to its end
//! (the user runs another statement, the app closes it at its spill limit, a cancel) is stopped
//! with `KILL QUERY`, and what the server still sends is read and dropped.
//!
//! **Statements before the last** of a run are read to their end without a limit (as psql and
//! the PostgreSQL driver do: a function or a lock in them takes effect for every row).
//!
//! **Transactions**: sessions run with `autocommit = 1`; the user's `START TRANSACTION`/`BEGIN`
//! opens one, and whether one is open is the server's status flag after every statement
//! (`SERVER_STATUS_IN_TRANS`), never a guess from the text: a statement that commits implicitly
//! (DDL, `LOCK TABLES`, another `BEGIN`, …) ends it, a deadlock rolls it back. After a failure
//! inside a transaction the session asks (`COM_PING`, whose answer has the flags). A statement
//! that ends an open transaction and opens another one (`BEGIN` inside one, `COMMIT AND CHAIN`)
//! is reported as the end of the one and the start of the other.
//!
//! **`EXPLAIN ANALYZE`** runs its statement to measure it and keeps nothing: outside the user's
//! transaction it runs in a transaction of its own that is rolled back, inside it under
//! `SAVEPOINT datarig_explain`, rolled back to and released afterwards (before the run answers).
//!
//! **Read-only sessions** (a read-only policy): the server's session is read-only
//! (`transaction_read_only`), which refuses writes while it is on. MySQL lets a statement turn it
//! off (`START TRANSACTION READ WRITE`, `SET SESSION transaction_read_only = OFF`, a procedure or
//! an executable comment that does, then turns it on again): the app refuses everything its
//! read-only policy does not let through before it is sent, and the session refuses it again
//! ([`DbError::ReadWriteRefused`]). After every statement it checks what the server says: a
//! session that is not read-only any more (the tracked variable), or a transaction that is not
//! read-only (the status flags), is rolled back and closed at once ([`DbError::ReadOnlyLost`]).
//! Only an account without write privileges is read-only whatever runs.
//!
//! **Cancel**: `KILL QUERY` of the session's connection from a connection of its own
//! (`session::Killer`), only while a statement of the session is on its way or running (`busy`);
//! a run of several statements also stops before its next one. A kill whose login comes after a
//! new run began is not sent.
//!
//! The session reports the sql mode its text is read in when it changes (`DbEvent::Language`),
//! and what the server said of a statement beyond its outcome (`DbEvent::Info`: the id an
//! `INSERT` generated, warnings, the server's own words, result sets it did not keep).

use crate::link::{Closed, Link, Next};
use crate::session::{Killer, Server, Tracked, my_error};
use crate::values::{column_meta, display};
use datarig_core::driver::{Cell, ColumnMeta, DbCommand, DbError, DbEvent, Outcome, PagingMode, StatementInfo};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::lexer::{Token, lex_in};
use datarig_core::sql::risk::mysql as risk;
use datarig_core::sql::risk::repeat::NotRepeatable;
use mysql_async::consts::StatusFlags;
use mysql_async::prelude::Queryable;
use mysql_async::{Column, Conn, QueryResult, Row, TextProtocol};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

/// ER_QUERY_INTERRUPTED: `KILL QUERY` stopped the statement.
const INTERRUPTED: u16 = 1317;

/// `sql_select_limit`'s value for no limit (its `DEFAULT`).
const NO_LIMIT: u64 = u64::MAX;

/// Rows read per request while a statement before the last, or skipped rows, are read.
const CHUNK: usize = 10_000;

/// The savepoint an `EXPLAIN ANALYZE` inside the user's transaction runs under.
const SAVEPOINT: &str = "datarig_explain";

/// How the query session runs what it is sent.
pub(crate) struct Settings {
    pub(crate) page_size: usize,
    /// The session is read-only (the server said so when it opened).
    pub(crate) read_only: bool,
}

/// What a statement run needs besides the connection.
struct Env<'a> {
    events: &'a UnboundedSender<DbEvent>,
    killer: &'a Killer,
    /// The UI asked to cancel (cleared when a run starts): a run of several statements stops
    /// before its next one.
    asked: &'a AtomicBool,
    /// A statement of the session is on its way to the server or running there.
    busy: &'a AtomicBool,
    page_size: usize,
    read_only: bool,
    server: Server,
}

/// What statements change in the session, as the server confirmed it.
struct State {
    tracked: Tracked,
    /// The mode last reported to the app (`DbEvent::Language`).
    mode: MySqlMode,
    limit: Limit,
    tx: Tx,
}

/// `sql_select_limit`: what the session has now (`None`: none), and what the user set
/// themselves (kept for the statements it applies to).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Limit {
    current: Option<u64>,
    user: Option<u64>,
}

/// The user's transaction, as the server's status flags say, and what the UI was last told.
#[derive(Default)]
struct Tx {
    open: bool,
    reported: bool,
}

/// A `sql_select_limit` value the server reported (`18446744073709551615`: none).
fn limit_value(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok().filter(|n| *n != NO_LIMIT)
}

impl Env<'_> {
    /// Tell the UI whether the user's transaction is open, when that changed (`Block`, then
    /// `TxOpen`: the session has no transaction of its own, so the two agree). `chained`: the
    /// statement ended one transaction and opened another, both said.
    fn report(&self, tx: &mut Tx, chained: bool) {
        if chained && tx.reported {
            let _ = self.events.send(DbEvent::Block(false));
            let _ = self.events.send(DbEvent::TxOpen(false));
            tx.reported = false;
        }
        if tx.reported != tx.open {
            tx.reported = tx.open;
            let _ = self.events.send(DbEvent::Block(tx.open));
            let _ = self.events.send(DbEvent::TxOpen(tx.open));
        }
    }

    /// Ask the server to stop what the session runs (`KILL QUERY`).
    async fn kill(&self) {
        self.killer.kill_query().await;
    }
}

/// The answer to one run (`Execute`, `Resume`). **Every run ends with exactly one terminal
/// event**, on every path: its first page (`Page` with columns), `Done`, or `Failed` (a cancel is
/// a `Failed` with `cancelled`). Each answer consumes the `Reply`; one dropped unanswered answers
/// with [`DbError::NoResult`] and trips a debug assertion; the run of a session the UI closed
/// answers nothing ([`Reply::closed`]). After a first page, a held result's later failure is the
/// one `Failed` that may still follow ([`Paged`]).
struct Reply<'a> {
    events: &'a UnboundedSender<DbEvent>,
    id: u64,
    answered: bool,
}

impl<'a> Reply<'a> {
    fn new(events: &'a UnboundedSender<DbEvent>, id: u64) -> Self {
        Self { events, id, answered: false }
    }

    fn page(mut self, columns: Vec<ColumnMeta>, rows: Vec<Vec<Cell>>, more: bool, elapsed: Duration) -> Paged<'a> {
        self.answered = true;
        let _ = self.events.send(DbEvent::Page { id: self.id, columns: Some(columns), rows, more, elapsed });
        Paged { events: self.events, id: self.id }
    }

    fn done(mut self, outcome: Outcome, elapsed: Duration) {
        self.answered = true;
        let _ = self.events.send(DbEvent::Done { id: self.id, outcome, elapsed });
    }

    fn fail(mut self, error: DbError, cancelled: bool) {
        self.answered = true;
        let _ = self.events.send(DbEvent::Failed { id: self.id, error, cancelled });
    }

    fn fail_with(self, e: &mysql_async::Error) {
        let (error, cancelled) = failure(e);
        self.fail(error, cancelled);
    }

    /// The UI closed the session: the run ends without an event.
    fn closed(mut self) {
        self.answered = true;
    }
}

impl Drop for Reply<'_> {
    fn drop(&mut self) {
        if !self.answered {
            let _ = self.events.send(DbEvent::Failed { id: self.id, error: DbError::NoResult, cancelled: false });
            debug_assert!(std::thread::panicking(), "run {} ended without a terminal event", self.id);
        }
    }
}

/// A run answered with its first page: one `Failed` may still follow, nothing else terminal.
struct Paged<'a> {
    events: &'a UnboundedSender<DbEvent>,
    id: u64,
}

impl Paged<'_> {
    fn fail_with(self, e: &mysql_async::Error) {
        let (error, cancelled) = failure(e);
        let _ = self.events.send(DbEvent::Failed { id: self.id, error, cancelled });
    }
}

/// Why a statement run stopped the session.
enum Stop {
    Closed,
    Lost(DbError),
}

impl From<Closed> for Stop {
    fn from(_: Closed) -> Self {
        Stop::Closed
    }
}

/// A run stopped by the session, with its reply when it was not answered yet.
struct Halted<'a> {
    stop: Stop,
    reply: Option<Reply<'a>>,
}

/// `?` for a step of a run: on a stop, hand the unanswered reply (`Some`, or `None` once it was
/// answered) back with it.
macro_rules! halt {
    ($e:expr, $reply:expr) => {
        match $e {
            Ok(v) => v,
            Err(stop) => return Err(Halted { stop: Stop::from(stop), reply: $reply }),
        }
    };
}

/// A client error as the run's failure, and whether it is a cancel (`KILL QUERY`).
fn failure(e: &mysql_async::Error) -> (DbError, bool) {
    match e {
        mysql_async::Error::Server(s) if s.code == INTERRUPTED => (DbError::Cancelled, true),
        e => (my_error(e), false),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn query_loop(
    mut conn: Conn,
    mut link: Link,
    killer: Killer,
    asked: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    events: UnboundedSender<DbEvent>,
    settings: Settings,
    server: Server,
    tracked: Tracked,
    select_limit: Option<u64>,
) {
    let env = Env {
        events: &events,
        killer: &killer,
        asked: &asked,
        busy: &busy,
        page_size: settings.page_size,
        read_only: settings.read_only,
        server,
    };
    let mode = tracked.mode(&server);
    // The limit the session set up with is its own, not the user's.
    let tracked = Tracked { select_limit: None, ..tracked };
    let mut s = State { tracked, mode, limit: Limit { current: select_limit, user: None }, tx: Tx::default() };
    let stop = loop {
        let done = match link.next().await {
            Next::Command(DbCommand::Execute { id, statements, paging }) => {
                execute(&mut conn, &mut link, &env, &mut s, id, statements, None, paging).await
            }
            Next::Command(DbCommand::Resume { id, sql, skip, paging }) => {
                execute(&mut conn, &mut link, &env, &mut s, id, vec![sql], Some(skip), paging).await
            }
            // Answered once; a session the UI closed meanwhile answers nothing.
            Next::Command(DbCommand::Count { id, sql }) => {
                count(&mut conn, &mut link, &env, &mut s, &sql).await.map(|(result, snapshot)| {
                    let _ = events.send(DbEvent::Counted { id, result, snapshot });
                })
            }
            Next::Command(DbCommand::CheckRepeat { id, sql }) => {
                check_repeat(&mut conn, &mut link, &env, &mut s, &sql).await.map(|result| {
                    let _ = events.send(DbEvent::RepeatChecked { id, result });
                })
            }
            // Nothing is held here (a held result serves its pages where it is read): an empty
            // last page, so the UI never waits for one.
            Next::Command(DbCommand::FetchMore { id }) => {
                let _ = events.send(DbEvent::Page {
                    id,
                    columns: None,
                    rows: Vec::new(),
                    more: false,
                    elapsed: Duration::ZERO,
                });
                Ok(())
            }
            Next::Command(_) => Ok(()),
            Next::Closed => Err(Stop::Closed),
            Next::Lost(reason) => Err(Stop::Lost(reason)),
        };
        if let Err(stop) = done {
            break stop;
        }
    };
    match stop {
        Stop::Lost(error) => {
            let _ = events.send(DbEvent::Lost { error });
        }
        Stop::Closed => crate::session::quit(conn).await,
    }
}

/// Run `statements` (or the one statement of a `Resume` past `skip` rows); the run ends with
/// exactly one terminal event ([`Reply`]).
#[allow(clippy::too_many_arguments)]
async fn execute(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    id: u64,
    statements: Vec<String>,
    resume: Option<u64>,
    paging: PagingMode,
) -> Result<(), Stop> {
    let reply = Reply::new(env.events, id);
    let ran = run(conn, link, env, s, reply, statements, resume, paging).await;
    env.busy.store(false, Ordering::SeqCst);
    match ran {
        Ok(()) => Ok(()),
        Err(Halted { stop, reply }) => {
            match (&stop, reply) {
                (Stop::Closed, Some(reply)) => reply.closed(),
                // The running statement fails first, then the session reports `Lost`.
                (Stop::Lost(error), Some(reply)) => reply.fail(error.clone(), false),
                (_, None) => {}
            }
            Err(stop)
        }
    }
}

/// One statement of a run.
#[derive(Clone, Copy)]
struct Step {
    last: bool,
    index: usize,
    start: Instant,
    /// Rows to read and drop before the first page (a `Resume`); 0 for a run.
    skip: u64,
    /// The last statement's result is held ([`PagingMode::Hold`]).
    hold: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run<'a>(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'a>,
    s: &mut State,
    reply: Reply<'a>,
    statements: Vec<String>,
    resume: Option<u64>,
    paging: PagingMode,
) -> Result<(), Halted<'a>> {
    let n = statements.len();
    if n == 0 {
        reply.done(Outcome::Command(String::new()), Duration::ZERO);
        return Ok(());
    }
    // A cancel asked for before this run belonged to the one before.
    env.asked.store(false, Ordering::SeqCst);
    // A statement run again for the user: what only the server can tell for the allowlist is
    // asked first.
    if resume.is_some()
        && let Err(why) = halt!(asked_server(conn, link, env, s, &statements[0]).await, Some(reply))
    {
        let cancelled = why == DbError::Cancelled;
        reply.fail(why, cancelled);
        return Ok(());
    }
    let mut reply = reply;
    for (i, sql) in statements.iter().enumerate() {
        let step = Step {
            last: i + 1 == n,
            index: i,
            start: Instant::now(),
            skip: resume.unwrap_or(0),
            hold: paging == PagingMode::Hold,
        };
        if n > 1 {
            // Cancelled between two statements: the rest does not run.
            if i > 0 && env.asked.load(Ordering::SeqCst) {
                reply.fail(DbError::Cancelled, true);
                return Ok(());
            }
            let _ = env.events.send(DbEvent::Started { id: reply.id, index: i });
        }
        reply = match statement(conn, link, env, s, reply, sql, step).await? {
            Some((reply, outcome)) => {
                let elapsed = step.start.elapsed();
                let _ = env.events.send(DbEvent::Finished { id: reply.id, index: i, outcome, elapsed });
                reply
            }
            None => return Ok(()),
        };
    }
    // The last statement always answers; a path that forgot is answered by the drop guard.
    drop(reply);
    Ok(())
}

/// How an `EXPLAIN ANALYZE` is undone: what goes before it, and after it.
struct Undo {
    before: String,
    after: Vec<String>,
}

impl Undo {
    /// Outside the user's transaction a transaction of its own, rolled back; inside it a
    /// savepoint, rolled back to and released. The row locks the statement took stay with the
    /// user's transaction until it ends (InnoDB keeps them past a rollback to a savepoint).
    fn of(open: bool) -> Self {
        if open {
            Undo {
                before: format!("SAVEPOINT {SAVEPOINT}"),
                after: vec![format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}"), format!("RELEASE SAVEPOINT {SAVEPOINT}")],
            }
        } else {
            Undo { before: "START TRANSACTION".to_string(), after: vec!["ROLLBACK".to_string()] }
        }
    }
}

/// What a statement left, read to the point the run answers.
enum Got {
    /// No rows: what its OK packet says.
    Done { outcome: Outcome, info: StatementInfo },
    /// Rows: the first page, whether more follow, and the result's columns.
    Rows { columns: Arc<[Column]>, page: Vec<Vec<Cell>>, more: bool, info: StatementInfo },
}

/// Run one statement of a run. The reply comes back with what the statement did when the run goes
/// on (a statement before the last that succeeded); `None` once the run was answered (its last
/// statement, or a failure).
#[allow(clippy::too_many_arguments)]
async fn statement<'a>(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'a>,
    s: &mut State,
    reply: Reply<'a>,
    sql: &str,
    step: Step,
) -> Result<Option<(Reply<'a>, Outcome)>, Halted<'a>> {
    let risk = risk::classify(sql, s.mode);
    // A read-only session runs only what the app's read-only policy lets through (the app
    // refuses the rest first): the server's read-only stops writes, not a procedure or an
    // executable comment that turns it off, writes, and turns it on again.
    if env.read_only && (risk.read_write || risk.read_only().is_err()) {
        reply.fail(DbError::ReadWriteRefused, false);
        return Ok(None);
    }
    let undo = (explains(sql, s.mode) && risk.rolls_back()).then(|| Undo::of(s.tx.open));
    // The result is held: the user's transaction, a held last statement, or a listing the app
    // cannot read on by running it again (`SHOW`, `DESCRIBE`): read to its end.
    let held = step.last && (step.hold || s.tx.open || lists(sql, s.mode));
    // The limit: a page and one row more (and the rows a `Resume` skips) for a last statement not
    // held, else the user's own; set only before a statement it applies to.
    let wanted = if step.last && !held {
        let page = (env.page_size as u64).saturating_add(1).saturating_add(step.skip);
        Some(s.limit.user.map_or(page, |u| u.min(page)))
    } else {
        s.limit.user
    };
    // Not before a statement that reads the outcome of the one before (`ROW_COUNT()`,
    // `FOUND_ROWS()`): the `SET` would be that statement.
    if limit_applies(sql, s.mode) && wanted != s.limit.current && !reads_outcome(sql, s.mode) {
        let set = format!("SET SESSION sql_select_limit = {}", wanted.map_or("DEFAULT".to_string(), |n| n.to_string()));
        if let Err(e) = halt!(link.guard(None, conn.query_drop(set)).await, Some(reply)) {
            return failed(conn, link, env, s, reply, &e, false).await;
        }
        s.tracked.update(conn);
        s.tracked.select_limit = None;
        s.limit.current = wanted;
    }
    if let Some(u) = &undo
        && let Err(e) = halt!(link.guard(None, conn.query_drop(u.before.clone())).await, Some(reply))
    {
        return failed(conn, link, env, s, reply, &e, false).await;
    }
    let was_open = s.tx.open;
    env.busy.store(true, Ordering::SeqCst);
    let sent = halt!(link.guard(Some(env.killer), conn.query_iter(sql)).await, Some(reply));
    let mut qr = match sent {
        Ok(qr) => qr,
        Err(e) => {
            env.busy.store(false, Ordering::SeqCst);
            if let Some(u) = &undo {
                halt!(undone(conn, link, u).await, Some(reply));
            }
            return failed(conn, link, env, s, reply, &e, was_open).await;
        }
    };
    let got = match qr.columns().filter(|c| !c.is_empty()) {
        None => {
            let outcome = outcome_of(sql, s.mode, qr.affected_rows());
            let mut info = info_of(&qr);
            // A procedure's further result sets (none have rows here, or they would be first).
            match halt!(link.guard(Some(env.killer), rest(&mut qr)).await, Some(reply)) {
                Ok(sets) => info.more_results += sets,
                Err(e) => {
                    drop(qr);
                    return failed(conn, link, env, s, reply, &e, was_open).await;
                }
            }
            Got::Done { outcome, info }
        }
        Some(columns) if !step.last => {
            // A statement before the last: every row, page by page, as its own result.
            let metas: Vec<ColumnMeta> = columns.iter().map(column_meta).collect();
            let mut first = true;
            loop {
                if env.asked.load(Ordering::SeqCst) {
                    halt!(stop_result(&mut qr, link, env).await, Some(reply));
                    drop(qr);
                    reply.fail(DbError::Cancelled, true);
                    return Ok(None);
                }
                match halt!(link.guard(Some(env.killer), read(&mut qr, &columns, CHUNK)).await, Some(reply)) {
                    Ok((rows, ended)) => {
                        let cols = first.then(|| metas.clone());
                        first = false;
                        let _ = env.events.send(DbEvent::StepRows {
                            id: reply.id,
                            index: step.index,
                            columns: cols,
                            rows,
                            more: !ended,
                        });
                        if ended {
                            break;
                        }
                    }
                    Err(e) => {
                        drop(qr);
                        return failed(conn, link, env, s, reply, &e, was_open).await;
                    }
                }
            }
            let mut info = info_of(&qr);
            match halt!(link.guard(Some(env.killer), rest(&mut qr)).await, Some(reply)) {
                Ok(sets) => info.more_results += sets,
                Err(e) => {
                    drop(qr);
                    return failed(conn, link, env, s, reply, &e, was_open).await;
                }
            }
            Got::Done { outcome: Outcome::Command(command_tag(sql, s.mode).0), info }
        }
        Some(columns) => {
            // The rows a `Resume` has already, read and dropped.
            let mut left = step.skip;
            while left > 0 {
                let want = usize::try_from(left).unwrap_or(usize::MAX).min(CHUNK);
                match halt!(link.guard(Some(env.killer), read(&mut qr, &columns, want)).await, Some(reply)) {
                    Ok((rows, ended)) => {
                        left = left.saturating_sub(rows.len() as u64);
                        if ended {
                            break;
                        }
                    }
                    Err(e) => {
                        drop(qr);
                        return failed(conn, link, env, s, reply, &e, was_open).await;
                    }
                }
            }
            let (mut page, ended) = if left > 0 {
                // The result now ends within them: an empty last page.
                (Vec::new(), true)
            } else {
                match halt!(link.guard(Some(env.killer), read(&mut qr, &columns, env.page_size + 1)).await, Some(reply))
                {
                    Ok(read) => read,
                    Err(e) => {
                        drop(qr);
                        return failed(conn, link, env, s, reply, &e, was_open).await;
                    }
                }
            };
            let more = page.len() > env.page_size;
            let carry = if more { page.pop() } else { None };
            if held && more {
                // Held: the statement runs on while the app reads it (or its last row is still
                // to come).
                let metas: Vec<ColumnMeta> = columns.iter().map(column_meta).collect();
                let paged = reply.page(metas, page, true, step.start.elapsed());
                let id = paged.id;
                let held = match hold(&mut qr, link, env, id, &columns, carry, ended).await {
                    Ok(held) => held,
                    Err(stop) => return Err(Halted { stop, reply: None }),
                };
                drop(qr);
                return held_ended(conn, link, env, s, paged, held, sql, was_open).await.map(|()| None);
            }
            // Not held: whatever the limit did not stop is read to its end and dropped.
            if !ended {
                match halt!(link.guard(Some(env.killer), drain(&mut qr)).await, Some(reply)) {
                    Ok(_) => {}
                    Err(e) => {
                        drop(qr);
                        return failed(conn, link, env, s, reply, &e, was_open).await;
                    }
                }
            }
            let mut info = info_of(&qr);
            match halt!(link.guard(Some(env.killer), rest(&mut qr)).await, Some(reply)) {
                Ok(sets) => info.more_results += sets,
                Err(e) => {
                    drop(qr);
                    return failed(conn, link, env, s, reply, &e, was_open).await;
                }
            }
            Got::Rows { columns, page, more, info }
        }
    };
    drop(qr);
    env.busy.store(false, Ordering::SeqCst);
    if let Some(u) = &undo {
        halt!(undone(conn, link, u).await, Some(reply));
    }
    // What the statement did to the session, before the run answers.
    if let Some(error) = halt!(after(conn, link, env, s, sql, risk.implicit_commit, was_open, true).await, Some(reply))
    {
        reply.fail(error.clone(), false);
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    let elapsed = step.start.elapsed();
    match got {
        Got::Done { outcome, info } => {
            send_info(env, reply.id, step.index, &info);
            if step.last {
                reply.done(outcome, elapsed);
                Ok(None)
            } else {
                Ok(Some((reply, outcome)))
            }
        }
        Got::Rows { columns, page, more, info } => {
            send_info(env, reply.id, step.index, &info);
            // Not held: nothing is open past the page.
            if more {
                let _ = env.events.send(DbEvent::Released { id: reply.id });
            }
            reply.page(columns.iter().map(column_meta).collect(), page, more, elapsed);
            Ok(None)
        }
    }
}

/// A statement failed (`e`): the run says so, and the session asks the server what it left
/// (whether the user's transaction is open: after a failure inside one, or a cancel). A broken
/// connection ends the session (`Lost` after the failure).
#[allow(clippy::too_many_arguments)]
async fn failed<'a>(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'a>,
    s: &mut State,
    reply: Reply<'a>,
    e: &mysql_async::Error,
    was_open: bool,
) -> Result<Option<(Reply<'a>, Outcome)>, Halted<'a>> {
    env.busy.store(false, Ordering::SeqCst);
    if e.is_fatal() {
        let error = my_error(e);
        reply.fail_with(e);
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    reply.fail_with(e);
    let cancelled = failure(e).1;
    if let Some(error) = halt!(after(conn, link, env, s, "", false, was_open || cancelled, false).await, None) {
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    Ok(None)
}

/// What a statement (`sql`; `ok`: it succeeded) left in the session, as the server says: the
/// session state it reports (the sql mode, read-only, a `sql_select_limit` the user set), and
/// whether the user's transaction is open (the status flags of the statement's answer, or, after
/// a failure when one may be open (`ask`), of a `COM_PING`). The UI is told what changed. On a
/// read-only session that is not read-only any more, the transaction is rolled back and the
/// error that ends the session comes back.
#[allow(clippy::too_many_arguments)]
async fn after(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    sql: &str,
    implicit_commit: bool,
    ask: bool,
    ok: bool,
) -> Result<Option<DbError>, Closed> {
    let mut status = if ok { conn.last_ok_packet().map(|p| p.status_flags()) } else { None };
    if ok {
        s.tracked.update(conn);
    }
    if status.is_none() && (ask || !ok) && (s.tx.open || ask) {
        status = match link.guard(None, conn.ping()).await? {
            Ok(()) => conn.last_ok_packet().map(|p| p.status_flags()),
            Err(_) => None,
        };
    }
    // A limit the user set (the session's own are taken where they are set).
    if let Some(v) = s.tracked.select_limit.take() {
        let limit = limit_value(&v);
        s.limit = Limit { current: limit, user: limit };
    }
    let mode = s.tracked.mode(&env.server);
    if mode != s.mode {
        s.mode = mode;
        let _ = env.events.send(DbEvent::Language(Language::Sql(Dialect::MySql(mode))));
    }
    if let Some(st) = status {
        let open = st.contains(StatusFlags::SERVER_STATUS_IN_TRANS);
        let chained = ok && s.tx.open && open && (implicit_commit || chains(sql, s.mode));
        s.tx.open = open;
        env.report(&mut s.tx, chained);
    }
    let writable = status.is_some_and(|st| {
        st.contains(StatusFlags::SERVER_STATUS_IN_TRANS) && !st.contains(StatusFlags::SERVER_STATUS_IN_TRANS_READONLY)
    });
    if env.read_only && (s.tracked.read_only == Some(false) || writable) {
        let _ = link.guard(None, conn.query_drop("ROLLBACK")).await?;
        return Ok(Some(DbError::ReadOnlyLost));
    }
    Ok(None)
}

/// Undo an `EXPLAIN ANALYZE` ([`Undo`]). A failure is not reported: only a broken connection
/// gets here, and the server rolls back the transaction of a connection that ends.
async fn undone(conn: &mut Conn, link: &mut Link, undo: &Undo) -> Result<(), Closed> {
    for sql in &undo.after {
        if link.guard(None, conn.query_drop(sql.clone())).await?.is_err() {
            break;
        }
    }
    Ok(())
}

/// How a held result ended ([`hold`]).
struct Held {
    /// It was stopped before its end (closed, or another command came).
    stopped: bool,
    /// Reading it failed (after its first page: the run's one `Failed` that may follow).
    failed: Option<mysql_async::Error>,
    /// What the server said of it, once it ended.
    info: StatementInfo,
    /// Commands that came meanwhile, to process next in their order.
    waiting: Vec<DbCommand>,
}

/// A held result: its first page was the answer; the next pages go to the app as it asks
/// (`FetchMore`) until the result ends, `ClosePortal` closes it, or another command comes (it is
/// processed next); a result stopped before its end is stopped on the server (`KILL QUERY`) and
/// read to its end. Counts and checks that come meanwhile wait for it.
async fn hold(
    qr: &mut QueryResult<'_, 'static, TextProtocol>,
    link: &mut Link,
    env: &Env<'_>,
    id: u64,
    columns: &[Column],
    mut carry: Option<Vec<Cell>>,
    mut ended: bool,
) -> Result<Held, Stop> {
    let page_size = env.page_size;
    let mut held = Held { stopped: false, failed: None, info: StatementInfo::default(), waiting: Vec::new() };
    let mut more = true;
    while more {
        match link.next_command().await {
            Next::Command(DbCommand::FetchMore { id: fid }) if fid == id => {
                let t0 = Instant::now();
                let want = page_size + 1 - usize::from(carry.is_some());
                let (rows, end) = if ended {
                    (Vec::new(), true)
                } else {
                    match link.guard(Some(env.killer), read(qr, columns, want)).await? {
                        Ok(read) => read,
                        Err(e) => {
                            held.failed = Some(e);
                            return Ok(held);
                        }
                    }
                };
                ended |= end;
                let mut rows: Vec<Vec<Cell>> = carry.take().into_iter().chain(rows).collect();
                more = rows.len() > page_size;
                if more {
                    carry = rows.pop();
                }
                let _ = env.events.send(DbEvent::Page { id, columns: None, rows, more, elapsed: t0.elapsed() });
            }
            // The app closes it (its spill limit, idle): it ends like a finished one.
            Next::Command(DbCommand::ClosePortal { id: cid }) if cid == id => break,
            Next::Command(c @ (DbCommand::Count { .. } | DbCommand::CheckRepeat { .. })) => held.waiting.push(c),
            Next::Command(DbCommand::FetchMore { .. } | DbCommand::ClosePortal { .. }) => {}
            Next::Command(other) => {
                held.waiting.insert(0, other);
                break;
            }
            Next::Closed => {
                let _ = tokio::time::timeout(Duration::from_secs(2), env.kill()).await;
                return Err(Stop::Closed);
            }
            Next::Lost(reason) => return Err(Stop::Lost(reason)),
        }
    }
    held.stopped = !ended;
    if held.stopped {
        stop_result(qr, link, env).await?;
        return Ok(held);
    }
    held.info = info_of(qr);
    match link.guard(Some(env.killer), rest(qr)).await? {
        Ok(sets) => held.info.more_results += sets,
        Err(e) => held.failed = Some(e),
    }
    Ok(held)
}

/// What a held result ([`Held`]) leaves: its failure or what the server said of it, the
/// session's state, and the commands that waited for it.
#[allow(clippy::too_many_arguments)]
async fn held_ended<'a>(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'a>,
    s: &mut State,
    paged: Paged<'a>,
    held: Held,
    sql: &str,
    was_open: bool,
) -> Result<(), Halted<'a>> {
    env.busy.store(false, Ordering::SeqCst);
    let id = paged.id;
    let ok = held.failed.is_none() && !held.stopped;
    if let Some(e) = &held.failed {
        paged.fail_with(e);
        if e.is_fatal() {
            return Err(Halted { stop: Stop::Lost(my_error(e)), reply: None });
        }
    } else if !held.stopped {
        send_info(env, id, 0, &held.info);
    }
    if let Some(error) = halt!(after(conn, link, env, s, sql, false, !ok || was_open, ok).await, None) {
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    link.requeue_all(held.waiting);
    Ok(())
}

/// Stop a result before its end: `KILL QUERY`, then read and drop what the server still sends
/// (its rows up to where it stopped, and the error that says so).
async fn stop_result(
    qr: &mut QueryResult<'_, 'static, TextProtocol>,
    link: &mut Link,
    env: &Env<'_>,
) -> Result<(), Closed> {
    env.kill().await;
    link.guard(Some(env.killer), async {
        loop {
            match qr.next().await {
                Ok(Some(_)) => {}
                Ok(None) if qr.is_empty() => return,
                Ok(None) => {}
                Err(_) => return,
            }
        }
    })
    .await?;
    Ok(())
}

/// Up to `max` rows of the current result set, as the app shows them, and whether the set
/// ended.
async fn read(
    qr: &mut QueryResult<'_, 'static, TextProtocol>,
    columns: &[Column],
    max: usize,
) -> Result<(Vec<Vec<Cell>>, bool), mysql_async::Error> {
    let mut rows = Vec::new();
    while rows.len() < max {
        match qr.next().await? {
            Some(row) => rows.push(cells(columns, &row)),
            None => return Ok((rows, true)),
        }
    }
    Ok((rows, false))
}

/// The rest of the current result set, read and dropped: how many rows.
async fn drain(qr: &mut QueryResult<'_, 'static, TextProtocol>) -> Result<u64, mysql_async::Error> {
    let mut n = 0;
    while qr.next().await?.is_some() {
        n += 1;
    }
    Ok(n)
}

/// The result sets after the current one (a procedure's), read and dropped: how many of them had
/// rows.
async fn rest(qr: &mut QueryResult<'_, 'static, TextProtocol>) -> Result<u32, mysql_async::Error> {
    let mut sets = 0;
    while !qr.is_empty() {
        if qr.columns().is_some_and(|c| !c.is_empty()) {
            sets += 1;
        }
        drain(qr).await?;
    }
    Ok(sets)
}

/// The values of `row` as the app shows them.
fn cells(columns: &[Column], row: &Row) -> Vec<Cell> {
    (0..row.len()).map(|i| columns.get(i).and_then(|c| display(c, row.as_ref(i)))).collect()
}

/// What the server said of the statement beyond its outcome, from its last OK packet (read only
/// once its result ended: before, it is the previous statement's).
fn info_of(qr: &QueryResult<'_, 'static, TextProtocol>) -> StatementInfo {
    let message = qr.info();
    StatementInfo {
        insert_id: qr.last_insert_id(),
        warnings: qr.warnings(),
        message: (!message.trim().is_empty()).then(|| message.into_owned()),
        more_results: 0,
    }
}

/// Send `info` of statement `index` of run `id`, when it says anything.
fn send_info(env: &Env<'_>, id: u64, index: usize, info: &StatementInfo) {
    if *info != StatementInfo::default() {
        let _ = env.events.send(DbEvent::Info { id, index, info: info.clone() });
    }
}

/// The words of `sql` (keywords and names, upper case), in MySQL mode `mode`.
fn words(sql: &str, mode: MySqlMode) -> Vec<String> {
    lex_in(sql, Dialect::MySql(mode))
        .into_iter()
        .filter(Token::is_word)
        .map(|t| t.text(sql).to_ascii_uppercase())
        .collect()
}

/// The first word of `sql` (upper case), in MySQL mode `mode`: `None` when it does not start with
/// one (an executable comment, a parenthesis).
fn first_word(sql: &str, mode: MySqlMode) -> Option<String> {
    lex_in(sql, Dialect::MySql(mode))
        .into_iter()
        .find(|t| !t.is_trivia())
        .filter(Token::is_word)
        .map(|t| t.text(sql).to_ascii_uppercase())
}

/// Whether `sql` is an `EXPLAIN` (or `DESCRIBE`/`DESC`, its other names).
fn explains(sql: &str, mode: MySqlMode) -> bool {
    matches!(first_word(sql, mode).as_deref(), Some("EXPLAIN" | "DESCRIBE" | "DESC"))
}

/// Whether the session's `sql_select_limit` applies to `sql`, from its first word: a query
/// (`SELECT`, `WITH`, `TABLE`, `VALUES`), a listing ([`lists`]), or text that does not start
/// with a word (a parenthesis, an executable comment). Only before such a statement is the limit
/// set: any other statement keeps the diagnostics of the one before (`SHOW WARNINGS` lists the
/// warnings of the last statement that is not a diagnostic one, and a `SET` of the session's
/// would be).
fn limit_applies(sql: &str, mode: MySqlMode) -> bool {
    match first_word(sql, mode) {
        None => true,
        Some(w) => matches!(w.as_str(), "SELECT" | "WITH" | "TABLE" | "VALUES") || lists(sql, mode),
    }
}

/// Whether `sql` is a listing the server cuts at `sql_select_limit` (`SHOW` but its diagnostics,
/// `DESCRIBE`/`DESC` of a table, not of a statement: that is an `EXPLAIN`): it is read without
/// the page limit, to its end.
fn lists(sql: &str, mode: MySqlMode) -> bool {
    let w = words(sql, mode);
    match w.first().map(String::as_str) {
        Some("DESCRIBE" | "DESC") => !matches!(
            w.get(1).map(String::as_str),
            Some(
                "ANALYZE"
                    | "FORMAT"
                    | "FOR"
                    | "SELECT"
                    | "WITH"
                    | "TABLE"
                    | "VALUES"
                    | "INSERT"
                    | "UPDATE"
                    | "DELETE"
                    | "REPLACE"
            )
        ),
        Some("SHOW") => !matches!(w.get(1).map(String::as_str), Some("WARNINGS" | "ERRORS" | "COUNT")),
        _ => false,
    }
}

/// Whether `sql` reads what the statement before it did (`ROW_COUNT()`, `FOUND_ROWS()`).
fn reads_outcome(sql: &str, mode: MySqlMode) -> bool {
    words(sql, mode).iter().any(|w| matches!(w.as_str(), "ROW_COUNT" | "FOUND_ROWS"))
}

/// Whether `sql` ends the transaction and opens another at once: `COMMIT`/`ROLLBACK … AND CHAIN`
/// (not `AND NO CHAIN`).
fn chains(sql: &str, mode: MySqlMode) -> bool {
    let w = words(sql, mode);
    matches!(w.first().map(String::as_str), Some("COMMIT" | "ROLLBACK"))
        && w.windows(2).any(|p| p[0] == "AND" && p[1] == "CHAIN")
}

/// The command a statement without rows ran, as its outcome names it (`CREATE TABLE`, `SET`), and
/// whether its count is of rows it changed (`INSERT`, `UPDATE`, `DELETE`, `REPLACE`, `LOAD`).
pub(crate) fn command_tag(sql: &str, mode: MySqlMode) -> (String, bool) {
    let words = words(sql, mode);
    let Some(first) = words.first() else { return (String::new(), false) };
    let dml = |w: &str| matches!(w, "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "LOAD");
    if dml(first) {
        return (first.clone(), true);
    }
    // `WITH … UPDATE`/`DELETE`.
    if first == "WITH"
        && let Some(w) = words.iter().find(|w| dml(w))
    {
        return (w.clone(), true);
    }
    if matches!(first.as_str(), "CREATE" | "DROP" | "ALTER") {
        // The kind of object, past what may come before it (`OR REPLACE`, `TEMPORARY`, `UNIQUE`,
        // `DEFINER = …`, `ALGORITHM = …`, `SQL SECURITY …`).
        let kinds = [
            "TABLE",
            "VIEW",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "TRIGGER",
            "EVENT",
            "DATABASE",
            "SCHEMA",
            "USER",
            "ROLE",
            "SERVER",
            "TABLESPACE",
            "LOGFILE",
            "INSTANCE",
            "RESOURCE",
            "SPATIAL",
        ];
        if let Some(obj) = words[1..].iter().find(|w| kinds.contains(&w.as_str())) {
            return (format!("{first} {obj}"), false);
        }
    }
    (first.clone(), false)
}

/// The outcome of a statement without rows that changed `affected` rows.
fn outcome_of(sql: &str, mode: MySqlMode, affected: u64) -> Outcome {
    let (tag, dml) = command_tag(sql, mode);
    if dml { Outcome::Affected(affected) } else { Outcome::Command(tag) }
}

/// A `SET_VAR` hint for the session's own catalog reads: they give up after 2 s behind a lock
/// another session holds or waits for, and stop after 10 s (MariaDB reads the hint as a comment).
const OWN_READ_HINTS: &str = "/*+ SET_VAR(lock_wait_timeout=2) MAX_EXECUTION_TIME(10000) */";

/// Ask the server what only it can tell for the allowlist about `sql`, right before the app runs
/// it again or counts it: whether a name it reads is a view (whose query may call anything) or a
/// table the server's own. `Ok` with the session's transaction isolation when nothing is in the
/// way; the refusal otherwise (a text off the allowlist is refused without asking).
async fn asked_server(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    sql: &str,
) -> Result<Result<String, DbError>, Closed> {
    let names = match risk::names(sql, s.mode) {
        Ok(names) => names,
        Err(why) => return Ok(Err(DbError::NotRepeatable(why))),
    };
    let ask = check_sql(&names, Dialect::MySql(s.mode));
    env.busy.store(true, Ordering::SeqCst);
    let answer = link.guard(Some(env.killer), conn.query_first::<(Option<String>, Option<String>), _>(ask)).await?;
    env.busy.store(false, Ordering::SeqCst);
    Ok(match answer {
        Ok(Some((_, Some(view)))) => Err(DbError::NotRepeatable(NotRepeatable::NotATable(view))),
        Ok(Some((isolation, None))) => Ok(isolation.unwrap_or_default()),
        Ok(None) => Err(DbError::NotRepeatable(NotRepeatable::Unreadable)),
        Err(e) => Err(failure(&e).0),
    })
}

/// The parts of a name as written (`` shop.`my table` ``, `"a.b"` under `ANSI_QUOTES`), unquoted,
/// lower case; `*` parts left out.
fn name_parts(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' | '"' => {
                while let Some(d) = chars.next() {
                    if d == c {
                        if chars.peek() == Some(&c) {
                            chars.next();
                            cur.push(c);
                        } else {
                            break;
                        }
                    } else {
                        cur.push(d);
                    }
                }
            }
            '.' => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    parts.into_iter().filter(|p| p != "*").map(|p| p.to_lowercase()).collect()
}

/// The question [`asked_server`] asks for `names` (every name a statement writes, as
/// `risk::mysql::names` gives them): the session's transaction isolation, and the first of them
/// that is a view or a table of the server's own (`information_schema.TABLES` rows other than
/// `BASE TABLE`), compared without case. A name may be a table of the current database, a
/// database's table (`db.t`), a column of a table (`t.c`, `db.t.c`) or an alias: every reading
/// that can name a table is asked about.
fn check_sql(names: &[String], d: Dialect) -> String {
    let mut bare: Vec<String> = Vec::new();
    let mut qualified: Vec<(String, String)> = Vec::new();
    for name in names {
        let parts = name_parts(name);
        match parts.as_slice() {
            [t] => bare.push(t.clone()),
            [a, b] => {
                qualified.push((a.clone(), b.clone()));
                bare.push(a.clone());
            }
            [a, b, ..] => qualified.push((a.clone(), b.clone())),
            [] => {}
        }
    }
    bare.sort();
    bare.dedup();
    qualified.sort();
    qualified.dedup();
    let lit = |s: &str| d.quote_literal(s);
    let mut cond = Vec::new();
    if !bare.is_empty() {
        let list: Vec<String> = bare.iter().map(|b| lit(b)).collect();
        cond.push(format!(
            "(LOWER(t.TABLE_SCHEMA) = LOWER(DATABASE()) AND LOWER(t.TABLE_NAME) IN ({}))",
            list.join(", ")
        ));
    }
    if !qualified.is_empty() {
        let list: Vec<String> = qualified.iter().map(|(a, b)| format!("({}, {})", lit(a), lit(b))).collect();
        cond.push(format!("(LOWER(t.TABLE_SCHEMA), LOWER(t.TABLE_NAME)) IN ({})", list.join(", ")));
    }
    let cond = if cond.is_empty() { "FALSE".to_string() } else { cond.join(" OR ") };
    format!(
        "SELECT {OWN_READ_HINTS} @@SESSION.transaction_isolation, (SELECT CONCAT(t.TABLE_SCHEMA, '.', t.TABLE_NAME) \
         FROM information_schema.TABLES t WHERE t.TABLE_TYPE <> 'BASE TABLE' AND ({cond}) LIMIT 1)"
    )
}

/// Count with `sql` (the app's `SELECT COUNT(*) FROM (…) AS datarig_count` of a statement on the
/// allowlist, checked again here): one statement, after the allowlist's question. Inside the
/// user's transaction at `REPEATABLE READ` or `SERIALIZABLE` it counts in the transaction's
/// snapshot (InnoDB's consistent read), which the answer says.
async fn count(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    sql: &str,
) -> Result<(Result<u64, DbError>, bool), Stop> {
    if let Err(why) = risk::repeatable(sql, s.mode) {
        return Ok((Err(DbError::NotRepeatable(why)), false));
    }
    let isolation = match asked_server(conn, link, env, s, sql).await? {
        Ok(isolation) => isolation,
        Err(why) => return Ok((Err(why), false)),
    };
    // The count's one row comes whatever the user's limit is (unless it is 0).
    if s.limit.current == Some(0) {
        if let Err(e) = link.guard(None, conn.query_drop("SET SESSION sql_select_limit = DEFAULT")).await? {
            return Ok((Err(my_error(&e)), false));
        }
        s.tracked.update(conn);
        s.tracked.select_limit = None;
        s.limit.current = None;
    }
    env.busy.store(true, Ordering::SeqCst);
    let counted = link.guard(Some(env.killer), conn.query_first::<String, _>(sql)).await?;
    env.busy.store(false, Ordering::SeqCst);
    let snapshot = s.tx.open && matches!(isolation.as_str(), "REPEATABLE-READ" | "SERIALIZABLE");
    let result = match counted {
        Ok(Some(n)) => n.trim().parse::<u64>().map_err(|_| DbError::NoResult),
        Ok(None) => Err(DbError::NoResult),
        Err(e) if e.is_fatal() => return Err(Stop::Lost(my_error(&e))),
        Err(e) => {
            let open = s.tx.open;
            if let Some(error) = after(conn, link, env, s, "", false, open, false).await? {
                return Err(Stop::Lost(error));
            }
            Err(failure(&e).0)
        }
    };
    Ok((result, snapshot))
}

/// The allowlist's question about `sql` alone (`DbCommand::CheckRepeat`).
async fn check_repeat(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    sql: &str,
) -> Result<Result<(), DbError>, Stop> {
    Ok(asked_server(conn, link, env, s, sql).await?.map(|_| ()))
}

#[cfg(test)]
mod tests;
