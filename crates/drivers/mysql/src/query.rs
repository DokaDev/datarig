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
//! **Manual commit** (`TxRule::Manual`): right before a statement the app flags as one that
//! opens a transaction, while none is open (the status flags, after the statements before it),
//! the session sends `START TRANSACTION` as a request of its own (multi-statements stay off: one
//! round trip more, on the first write of each transaction only) and the statement after it; a
//! statement that commits implicitly (DDL, `LOCK TABLES`, …) gets none, it would end it at once.
//! `autocommit` stays on, so reads after the transaction ends are autocommit again. The
//! transaction is reported (`ManualBegun`, then `Block`) once the statement succeeded; when the
//! run fails or is cancelled in it, it is rolled back at once (`ManualRolledBack`). In a
//! transaction open before the run, a failure undoes only the failed statement (the server's
//! rule; no savepoint is set for the run, which would cost a round trip): the statements of the
//! run before it keep what they did, and the transaction stays open. A deadlock (`1213`) is the
//! server rolling back the whole transaction, and an implicit commit its committing it: said
//! (`TxClosedBy`) before the status flags close the block. A transaction switched inside the run
//! that opened one (an implicit commit, `AND CHAIN`, a procedure) is never rolled back by the
//! session. `EndTx` sends `COMMIT` or `ROLLBACK` (`AND NO CHAIN NO RELEASE`, whatever the
//! session's `completion_type`); a connection that ends meanwhile leaves a commit's outcome
//! unknown. A user's `SET autocommit = 0` is reported
//! (`DbEvent::Autocommit`, from `SERVER_STATUS_AUTOCOMMIT`): every statement then opens a
//! transaction, which the flags show open.
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
//! new run began is not sent. A kill the server takes is waited for, however long the statement
//! then takes to roll back. MySQL refuses one of a statement that runs as another account
//! (inside a `SQL SECURITY DEFINER` routine), even after a few tries: such a statement that
//! sends nothing runs on to its end whatever the session does (the server does not look at its
//! connection meanwhile), so the session waits for it and says why ([`DbEvent::CancelRefused`]).
//! Where reading on could last forever, the connection is cut instead: a refused statement whose
//! rows keep coming, a kill that cannot be sent or is sent and never answered, a taken one whose
//! rows still keep coming long after. The run ends cancelled, what waited is answered, and the
//! session is lost ([`DbError::CancelNotTaken`], with why). Whether the
//! statement then stops is the server's: one that sends finds the connection gone. A result
//! the session stops itself (a held one) is stopped so too. A kill sent again that never gets
//! its answer while the statement answers may still land on whatever the connection runs next:
//! the statement's answer is read to its end and is the run's, nothing more is sent on the
//! connection, and the session is lost after it ([`CancelFailure::StopPending`]). So too after
//! the user's own kill, written and never answered while the statement ended: nothing is sent
//! after the statement until the kill is answered (an `EXPLAIN ANALYZE`'s rollback neither: the
//! server rolls it back as the connection ends, and no transaction is said open meanwhile).
//!
//! The session reports the sql mode its text is read in when it changes (`DbEvent::Language`),
//! and what the server said of a statement beyond its outcome (`DbEvent::Info`: the id an
//! `INSERT` generated, warnings, the server's own words, result sets it did not keep).

use crate::link::{Closed, Link, Next};
use crate::session::{Gate, Killer, Server, Tracked, my_error};
use crate::values::{column_meta, display};
use datarig_core::driver::{
    CancelFailure, Cell, ColumnMeta, DbCommand, DbError, DbEvent, Outcome, PagingMode, ServerEnd, SessionValues,
    StatementInfo, TxOutcome, TxRule, TxStep,
};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::lexer::{Tok, Token, lex_in};
use datarig_core::sql::risk::Class;
use datarig_core::sql::risk::mysql as risk;
use datarig_core::sql::risk::repeat::NotRepeatable;
use futures::future::BoxFuture;
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

/// How the session opens a transaction for manual commit.
const START: &str = "START TRANSACTION";

/// How the session ends a transaction itself: never chained nor ending the session, whatever the
/// session's `completion_type`.
const COMMIT: &str = "COMMIT AND NO CHAIN NO RELEASE";
const ROLLBACK: &str = "ROLLBACK AND NO CHAIN NO RELEASE";

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
    /// The run going on answered with a failure (cleared when a run starts; [`Reply`]).
    failed: &'a AtomicBool,
    page_size: usize,
    read_only: bool,
    server: Server,
}

/// What statements change in the session, as the server confirmed it.
struct State {
    tracked: Tracked,
    /// The mode last reported to the app (`DbEvent::Language`).
    mode: MySqlMode,
    /// What the values are read in, as last reported to the app (`DbEvent::SessionValues`).
    values: Option<SessionValues>,
    /// The statement running now may run code of the server's (a procedure, a stored function):
    /// what that code changes in the session (`SET time_zone`, `SET NAMES`, `SET sql_mode`) the
    /// server does not report, so the session asks once it ended ([`after`]).
    routine: bool,
    limit: Limit,
    tx: Tx,
    /// The request being served (a run, an `EndTx`, a count), whose id a server's end of the
    /// transaction is said with ([`DbEvent::TxClosedBy`]).
    request: u64,
}

/// `sql_select_limit`: what the session has now (`None`: none), and what the user set
/// themselves (kept for the statements it applies to).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Limit {
    current: Option<u64>,
    user: Option<u64>,
}

/// The user's transaction, as the server's status flags say, and what the UI was last told.
struct Tx {
    open: bool,
    reported: bool,
    /// The session runs with autocommit (`SERVER_STATUS_AUTOCOMMIT`), as last reported.
    autocommit: bool,
    /// The transaction was opened for manual commit right before statement `.1` of run `.0`:
    /// reported (`ManualBegun`) once that statement succeeded.
    opening: Option<(u64, usize)>,
    /// Statement `index` of the run going on opened the transaction for manual commit: when the
    /// run fails, it is rolled back ([`settle`]).
    opened: Option<usize>,
}

impl Default for Tx {
    fn default() -> Self {
        // A session starts with autocommit on (it is set up so).
        Tx { open: false, reported: false, autocommit: true, opening: None, opened: None }
    }
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
        // A transaction opened for manual commit is one once its statement succeeded.
        if let Some((id, index)) = tx.opening.take()
            && tx.open
        {
            let _ = self.events.send(DbEvent::ManualBegun { id, index });
        }
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
    /// Set when it answers with a failure ([`Env::failed`]).
    failed: &'a AtomicBool,
    id: u64,
    answered: bool,
}

impl<'a> Reply<'a> {
    fn new(events: &'a UnboundedSender<DbEvent>, failed: &'a AtomicBool, id: u64) -> Self {
        Self { events, failed, id, answered: false }
    }

    fn page(mut self, columns: Vec<ColumnMeta>, rows: Vec<Vec<Cell>>, more: bool, elapsed: Duration) -> Paged<'a> {
        self.answered = true;
        let _ = self.events.send(DbEvent::Page { id: self.id, columns: Some(columns), rows, more, elapsed });
        Paged { events: self.events, failed: self.failed, id: self.id }
    }

    fn done(mut self, outcome: Outcome, elapsed: Duration) {
        self.answered = true;
        let _ = self.events.send(DbEvent::Done { id: self.id, outcome, elapsed });
    }

    fn fail(mut self, error: DbError, cancelled: bool) {
        self.answered = true;
        self.failed.store(true, Ordering::SeqCst);
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
            self.failed.store(true, Ordering::SeqCst);
            let _ = self.events.send(DbEvent::Failed { id: self.id, error: DbError::NoResult, cancelled: false });
            // Nothing is owed to nobody: the session's task is dropped once its UI is gone (the
            // runtime shutting down mid-run).
            debug_assert!(
                std::thread::panicking() || self.events.is_closed(),
                "run {} ended without a terminal event",
                self.id
            );
        }
    }
}

/// A run answered with its first page: one `Failed` may still follow, nothing else terminal.
struct Paged<'a> {
    events: &'a UnboundedSender<DbEvent>,
    failed: &'a AtomicBool,
    id: u64,
}

impl Paged<'_> {
    fn fail_with(self, e: &mysql_async::Error) {
        let (error, cancelled) = failure(e);
        self.failed.store(true, Ordering::SeqCst);
        let _ = self.events.send(DbEvent::Failed { id: self.id, error, cancelled });
    }

    /// The cancel stopped it after its first page.
    fn cancelled(self) {
        self.failed.store(true, Ordering::SeqCst);
        let _ = self.events.send(DbEvent::Failed { id: self.id, error: DbError::Cancelled, cancelled: true });
    }
}

/// Why a statement run stopped the session.
enum Stop {
    Closed,
    Lost(DbError),
}

impl From<Closed> for Stop {
    fn from(c: Closed) -> Self {
        match c {
            Closed::Ui => Stop::Closed,
            Closed::CancelNotTaken(why) => Stop::Lost(DbError::CancelNotTaken(why)),
        }
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
    let failed = AtomicBool::new(false);
    let env = Env {
        events: &events,
        killer: &killer,
        asked: &asked,
        busy: &busy,
        failed: &failed,
        page_size: settings.page_size,
        read_only: settings.read_only,
        server,
    };
    let mode = tracked.mode(&server);
    let values = tracked.values();
    // The limit the session set up with is its own, not the user's.
    let tracked = Tracked { select_limit: None, ..tracked };
    let mut s = State {
        tracked,
        mode,
        values,
        routine: false,
        limit: Limit { current: select_limit, user: None },
        tx: Tx::default(),
        request: 0,
    };
    let stop = loop {
        let done = match link.next().await {
            Next::Command(DbCommand::Execute { id, statements, paging, tx }) => {
                // MySQL has no idle limit for a transaction (`idle_timeout`), nor a read-only
                // transaction that costs no round trip of its own: a `ReadOnly` step runs as it
                // is (a write hidden behind a view's stored function commits by itself).
                let opens: Vec<bool> = match tx {
                    TxRule::Auto => Vec::new(),
                    TxRule::Manual { steps, .. } => steps.iter().map(|step| *step == TxStep::Open).collect(),
                };
                execute(&mut conn, &mut link, &env, &mut s, id, statements, None, paging, &opens).await
            }
            Next::Command(DbCommand::Resume { id, sql, skip, paging }) => {
                execute(&mut conn, &mut link, &env, &mut s, id, vec![sql], Some(skip), paging, &[]).await
            }
            // Answered once; a session the UI closed meanwhile answers nothing.
            Next::Command(DbCommand::EndTx { id, commit }) => {
                end_tx(&mut conn, &mut link, &env, &mut s, id, commit).await
            }
            // Answered once; a session the UI closed meanwhile answers nothing.
            // A count or a check the app asks for: a cancel asked before it was another run's.
            Next::Command(DbCommand::Count { id, sql }) => {
                link.serving(&events, id);
                s.request = id;
                env.asked.store(false, Ordering::SeqCst);
                killer.clear();
                count(&mut conn, &mut link, &env, &mut s, &sql).await.map(|(result, snapshot)| {
                    let _ = events.send(DbEvent::Counted { id, result, snapshot });
                })
            }
            Next::Command(DbCommand::CheckRepeat { id, sql }) => {
                link.serving(&events, id);
                s.request = id;
                env.asked.store(false, Ordering::SeqCst);
                killer.clear();
                check_repeat(&mut conn, &mut link, &env, &mut s, &sql).await.map(|result| {
                    let _ = events.send(DbEvent::RepeatChecked { id, result });
                })
            }
            // Answered once, after its last batch; a session the UI closed meanwhile answers
            // nothing, a lost one answers what waited for it.
            Next::Command(DbCommand::Export { id, sql, guard, sink, batch }) => {
                link.serving(&events, id);
                export::export(&mut conn, &mut link, &env, &mut s, &sql, guard, sink, batch).await.map(
                    |(result, cancelled)| {
                        let _ = events.send(DbEvent::Exported { id, result, cancelled });
                    },
                )
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
            let error = link.lost_as(error);
            // What waited for the session is answered first: no request is left without one.
            for c in link.take_queued() {
                unanswered(&events, c, &error);
            }
            let _ = events.send(DbEvent::Lost { error });
        }
        Stop::Closed => crate::session::quit(conn).await,
    }
}

/// Answer a request the session took and did not get to before it was lost (`error`, why): a
/// run fails with it, as a running statement does; a count or a check gets it as its result.
fn unanswered(events: &UnboundedSender<DbEvent>, c: DbCommand, error: &DbError) {
    let ev = match c {
        DbCommand::Execute { id, .. } | DbCommand::Resume { id, .. } | DbCommand::EndTx { id, .. } => {
            DbEvent::Failed { id, error: error.clone(), cancelled: false }
        }
        DbCommand::Count { id, .. } => DbEvent::Counted { id, result: Err(error.clone()), snapshot: false },
        DbCommand::CheckRepeat { id, .. } => DbEvent::RepeatChecked { id, result: Err(error.clone()) },
        DbCommand::Export { id, .. } => DbEvent::Exported { id, result: Err(error.clone()), cancelled: false },
        // Nothing to answer: a page of a result that is gone, a close of one.
        _ => return,
    };
    let _ = events.send(ev);
}

/// Run `statements` (or the one statement of a `Resume` past `skip` rows; `opens`: the ones
/// that open a transaction for manual commit, [`TxRule::Manual`]); the run ends with exactly one
/// terminal event ([`Reply`]).
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
    opens: &[bool],
) -> Result<(), Stop> {
    link.serving(env.events, id);
    s.request = id;
    env.failed.store(false, Ordering::SeqCst);
    let reply = Reply::new(env.events, env.failed, id);
    let ran = run(conn, link, env, s, reply, statements, resume, paging, opens).await;
    idle(env, link).await;
    let ran = match ran {
        Ok(()) => settle(conn, link, env, s, id).await,
        Err(halted) => {
            // The connection is gone or runs nothing more: the server rolls back what is open.
            (s.tx.opening, s.tx.opened) = (None, None);
            Err(halted)
        }
    };
    match ran {
        Ok(()) => Ok(()),
        Err(Halted { stop, reply }) => {
            match (&stop, reply) {
                (Stop::Closed, Some(reply)) => reply.closed(),
                // Only a kill gets the connection cut so: the run was cancelled.
                (Stop::Lost(DbError::CancelNotTaken(_)), Some(reply)) => reply.fail(DbError::Cancelled, true),
                // The running statement fails first, then the session reports `Lost`.
                (Stop::Lost(error), Some(reply)) => reply.fail(error.clone(), false),
                // The run was answered already (its first page, or its failure): `halt!` hands
                // the reply back whenever it was not, so nothing more is owed.
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
    /// It opens a transaction for manual commit when none is open ([`TxRule::Manual`]).
    opens: bool,
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
    opens: &[bool],
) -> Result<(), Halted<'a>> {
    let n = statements.len();
    if n == 0 {
        reply.done(Outcome::Command(String::new()), Duration::ZERO);
        return Ok(());
    }
    // A cancel asked for before this run belonged to the one before.
    env.asked.store(false, Ordering::SeqCst);
    env.killer.clear();
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
            opens: opens.get(i) == Some(&true),
        };
        if n > 1 {
            // Cancelled between two statements: the rest does not run. Nor on a connection that
            // runs nothing more (a kill sent again for the statement before never answered,
            // which only a cancel sends).
            if i > 0 && (env.asked.load(Ordering::SeqCst) || link.tainted()) {
                reply.fail(DbError::Cancelled, true);
                return Ok(());
            }
            let _ = env.events.send(DbEvent::Started { id: reply.id, index: i });
        }
        // A transaction opened earlier in the run that a statement of the user's ended (`INSERT;
        // COMMIT; …`): nothing of the run is left to undo; a later write opens a new one.
        if !s.tx.open {
            s.tx.opened = None;
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
    s.routine = runs_server_code(sql, s.mode, risk.runs_code);
    // A read-only session runs only what the app's read-only policy lets through (the app
    // refuses the rest first): the server's read-only stops writes, not a procedure or an
    // executable comment that turns it off, writes, and turns it on again.
    if env.read_only && (risk.read_write || risk.read_only().is_err()) {
        reply.fail(DbError::ReadWriteRefused, false);
        return Ok(None);
    }
    // Manual commit: a transaction for the statement, as a request of its own. Not for one that
    // commits implicitly (it would end it at once) nor for transaction control of the user's.
    if step.opens && !s.tx.open && !risk.implicit_commit && risk.class != Class::Tx {
        if let Err(e) = halt!(link.guard(None, conn.query_drop(START)).await, Some(reply)) {
            return failed(conn, link, env, s, reply, &e, false).await;
        }
        s.tracked.update(conn);
        let flags = conn.last_ok_packet().map(|p| p.status_flags());
        if !flags.is_some_and(|f| f.contains(StatusFlags::SERVER_STATUS_IN_TRANS)) {
            // Never run as autocommit what was to be rolled back on asking: nothing is sent.
            reply.fail(DbError::TxNotOpened, false);
            if let Some(error) = halt!(after(conn, link, env, s, START, false, false, true).await, None) {
                return Err(Halted { stop: Stop::Lost(error), reply: None });
            }
            return Ok(None);
        }
        s.tx.open = true;
        s.tx.opening = Some((reply.id, step.index));
        s.tx.opened = Some(step.index);
        // Cancelled while it opened: the statement is not sent, and the run's end rolls back.
        if env.asked.load(Ordering::SeqCst) {
            reply.fail(DbError::Cancelled, true);
            return Ok(None);
        }
    }
    let undo = (explains(sql, s.mode) && risk.rolls_back()).then(|| Undo::of(s.tx.open));
    // The result is held: the user's transaction, a held last statement, a listing the app
    // cannot read on by running it again (`SHOW`, `DESCRIBE`), or a locking read (the limit
    // would lock fewer rows than asked, and its next page would lock them again): read to its
    // end.
    let held = step.last && (step.hold || s.tx.open || lists(sql, s.mode) || locking(sql, s.mode));
    // The limit: a page and one row more (and the rows a `Resume` skips) for a last statement not
    // held, else the user's own; set only before a statement it applies to.
    let wanted = if step.last && !held {
        let page = (env.page_size as u64).saturating_add(1).saturating_add(step.skip);
        Some(s.limit.user.map_or(page, |u| u.min(page)))
    } else {
        s.limit.user
    };
    // Not before a statement that reads the outcome of the one before (`ROW_COUNT()`,
    // `FOUND_ROWS()`): the `SET` would be that statement. It then runs under the limit it finds
    // only when that one cuts no row it wants (none, or at least as many).
    let keeps = match (s.limit.current, wanted) {
        (None, _) => true,
        (Some(now), Some(want)) => now >= want,
        (Some(_), None) => false,
    };
    let skip = keeps && reads_outcome(sql, s.mode);
    if limit_applies(sql, s.mode) && wanted != s.limit.current && !skip {
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
            idle(env, link).await;
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
            if s.routine {
                let _ = env.events.send(DbEvent::ValuesNotKnown { id: reply.id });
            }
            loop {
                if env.asked.load(Ordering::SeqCst) {
                    halt!(stop_result(&mut qr, link, env).await, Some(reply));
                    drop(qr);
                    forget_after_code(env, s);
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
                if s.routine {
                    let _ = env.events.send(DbEvent::ValuesNotKnown { id: reply.id });
                }
                // The transaction opened for it is open while the app reads the result (said
                // from `START TRANSACTION`'s answer; its end is said once the result ended).
                if s.tx.opening.is_some() {
                    env.report(&mut s.tx, false);
                }
                let paged = reply.page(metas, page, true, step.start.elapsed());
                let id = paged.id;
                let held = match hold(&mut qr, link, env, id, &columns, carry, ended).await {
                    Ok(held) => held,
                    Err(stop) => {
                        // A cancel the server did not take: the run's one failure that follows.
                        if matches!(stop, Stop::Lost(DbError::CancelNotTaken(_))) && env.asked.load(Ordering::SeqCst) {
                            paged.cancelled();
                        }
                        return Err(Halted { stop, reply: None });
                    }
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
    idle(env, link).await;
    let kept = match &undo {
        Some(u) => halt!(undone(conn, link, u).await, Some(reply)),
        None => true,
    };
    // More than one result set: a procedure ran, whatever the text looks like.
    let sets = match &got {
        Got::Done { info, .. } | Got::Rows { info, .. } => info.more_results,
    };
    s.routine |= sets > 0;
    let code_ran = s.routine;
    // What the statement did to the session, before the run answers. An undo that failed on a
    // connection still in use may have left the statement's transaction open: the server is
    // asked (one that runs nothing more is not).
    let ask = was_open || !kept;
    if let Some(error) = halt!(after(conn, link, env, s, sql, risk.implicit_commit, ask, kept).await, Some(reply)) {
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
            if code_ran {
                let _ = env.events.send(DbEvent::ValuesNotKnown { id: reply.id });
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
    idle(env, link).await;
    if e.is_fatal() {
        let error = my_error(e);
        reply.fail_with(e);
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    reply.fail_with(e);
    let cancelled = failure(e).1;
    // Without autocommit the failed statement may have opened a transaction: asked too.
    let ask = was_open || cancelled || !s.tx.autocommit;
    if let Some(error) = halt!(after(conn, link, env, s, "", false, ask, false).await, None) {
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    Ok(None)
}

/// What a statement (`sql`; `ok`: it succeeded, and its answer says what it left: not an
/// `EXPLAIN ANALYZE` whose undo was cut off) left in the session, as the server says: the
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
    // Code of the server's that the statement ran (a procedure, a stored function) may have
    // changed the session without the server reporting it: asked, whether it succeeded or not.
    // A connection that runs nothing more is not asked: what the statement did is answered all
    // the same, and the session ends after it.
    let routine = std::mem::take(&mut s.routine);
    if routine && link.tainted() {
        s.tracked.forget_values();
    } else if routine {
        let asked: Option<(String, String, String, String)> =
            match link.guard(None, conn.query_first(crate::session::ASK_VALUES)).await? {
                Ok(row) => row,
                Err(e) if e.is_fatal() => return Ok(Some(my_error(&e))),
                Err(_) => None,
            };
        match asked {
            Some(row) => s.tracked.set_asked(row),
            // Not known: the app refuses what depends on them until the session says.
            None => s.tracked.forget_values(),
        }
    }
    if status.is_none() && (ask || !ok) && (s.tx.open || ask) && !link.tainted() {
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
    let values = s.tracked.values();
    if values.is_some() && values != s.values {
        s.values = values.clone();
        if let Some(v) = values {
            let _ = env.events.send(DbEvent::SessionValues(v));
        }
    }
    if let Some(st) = status {
        let open = st.contains(StatusFlags::SERVER_STATUS_IN_TRANS);
        let was_open = s.tx.open;
        let switches = ok && (implicit_commit || chains(sql, s.mode));
        let chained = was_open && open && switches;
        // The transaction the run opened is not the one open any more, or may not be (a
        // procedure that commits and starts another): the run's end never rolls back what is
        // open now (committed work, the user's own block).
        if ok && was_open && (switches || routine) {
            s.tx.opened = None;
        }
        if switches {
            s.tx.opening = None;
        }
        // The server ended the transaction the UI knows open: a statement committed it
        // implicitly, or a failure rolled all of it back (a deadlock).
        if s.tx.reported && was_open && (!open || chained) {
            let how = if !ok && !open {
                Some(ServerEnd::RolledBack)
            } else if ok && implicit_commit {
                Some(ServerEnd::Committed)
            } else {
                None
            };
            if let Some(how) = how {
                let _ = env.events.send(DbEvent::TxClosedBy { id: s.request, how });
            }
        }
        s.tx.open = open;
        let autocommit = st.contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT);
        if autocommit != s.tx.autocommit {
            s.tx.autocommit = autocommit;
            let _ = env.events.send(DbEvent::Autocommit(autocommit));
        }
        // A transaction opened for the run, which failed in it before it was reported, is never
        // reported: it is rolled back once the run ends ([`settle`]).
        if s.tx.opening.is_none() || !env.failed.load(Ordering::SeqCst) {
            env.report(&mut s.tx, chained);
        }
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

/// After run `id`: a run that failed or was cancelled in the transaction a statement of it opened
/// for manual commit ([`Tx::opened`]) rolls that transaction back (`ROLLBACK`, then
/// [`DbEvent::ManualRolledBack`] and the state): nothing the run did in it is kept. A
/// transaction the server ended already (a deadlock) is not rolled back again, and one on a
/// connection that runs nothing more is rolled back by the server as the connection ends.
async fn settle<'a>(conn: &mut Conn, link: &mut Link, env: &Env<'a>, s: &mut State, id: u64) -> Result<(), Halted<'a>> {
    s.tx.opening = None;
    let Some(index) = s.tx.opened.take() else { return Ok(()) };
    if !env.failed.load(Ordering::SeqCst) || !s.tx.open || link.tainted() {
        return Ok(());
    }
    let error = match halt!(link.guard(None, conn.query_drop(ROLLBACK)).await, None) {
        Ok(()) => {
            let _ = env.events.send(DbEvent::ManualRolledBack { id, index });
            halt!(after(conn, link, env, s, ROLLBACK, false, false, true).await, None)
        }
        Err(e) if e.is_fatal() => Some(my_error(&e)),
        Err(_) => {
            // Refused: the transaction may still be open, with what the run did in it. Said as
            // the one opened for manual commit (`ManualBegun` first), never as the user's.
            if !s.tx.reported {
                s.tx.opening = Some((id, index));
                env.failed.store(false, Ordering::SeqCst);
            }
            halt!(after(conn, link, env, s, "", false, true, false).await, None)
        }
    };
    match error {
        Some(error) => Err(Halted { stop: Stop::Lost(error), reply: None }),
        None => Ok(()),
    }
}

/// End the user's transaction (`DbCommand::EndTx` `id`; one the user opened, or the session for
/// manual commit): `COMMIT` (`commit`) or `ROLLBACK` as a request of its own, answered with
/// exactly one `TxEnded` or `Failed`, then the state from the answer's status flags. With none
/// open, nothing is sent ([`DbError::NoTransaction`]). A connection that ends meanwhile leaves a
/// commit's outcome unknown ([`DbError::CommitUnknown`]), and the session is lost after it.
async fn end_tx(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    id: u64,
    commit: bool,
) -> Result<(), Stop> {
    if !s.tx.open {
        let _ = env.events.send(DbEvent::Failed { id, error: DbError::NoTransaction, cancelled: false });
        return Ok(());
    }
    s.request = id;
    let sql = if commit { COMMIT } else { ROLLBACK };
    let unknown = |e: &mysql_async::Error| if commit { DbError::CommitUnknown } else { my_error(e) };
    let error = match link.guard(None, conn.query_drop(sql)).await? {
        Ok(()) => {
            let outcome = if commit { TxOutcome::Committed } else { TxOutcome::RolledBack };
            let _ = env.events.send(DbEvent::TxEnded { id, outcome });
            after(conn, link, env, s, sql, false, false, true).await?
        }
        Err(e) if e.is_fatal() => {
            let _ = env.events.send(DbEvent::Failed { id, error: unknown(&e), cancelled: false });
            Some(my_error(&e))
        }
        // Refused: the server says what it left. A connection that does not answer that leaves
        // a commit's outcome unknown.
        Err(e) => match link.guard(None, conn.ping()).await? {
            Ok(()) => {
                let _ = env.events.send(DbEvent::Failed { id, error: my_error(&e), cancelled: false });
                let open = conn
                    .last_ok_packet()
                    .is_some_and(|p| p.status_flags().contains(StatusFlags::SERVER_STATUS_IN_TRANS));
                if !open && s.tx.reported {
                    let _ = env.events.send(DbEvent::TxClosedBy { id, how: ServerEnd::RolledBack });
                }
                after(conn, link, env, s, "", false, false, true).await?
            }
            Err(dead) => {
                let _ = env.events.send(DbEvent::Failed { id, error: unknown(&e), cancelled: false });
                Some(my_error(&dead))
            }
        },
    };
    match error {
        Some(error) => Err(Stop::Lost(error)),
        None => Ok(()),
    }
}

/// Undo an `EXPLAIN ANALYZE` ([`Undo`]): whether it was. A failure is not reported: only a
/// broken connection (or one that runs nothing more, [`Link::taint`]) gets here, and the server
/// rolls back the transaction of a connection that ends.
async fn undone(conn: &mut Conn, link: &mut Link, undo: &Undo) -> Result<bool, Closed> {
    for sql in &undo.after {
        if link.guard(None, conn.query_drop(sql.clone())).await?.is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The session's statement ended ([`Killer::idle`]): a kill of the user's sent for it that never
/// got its answer may still be carried out, on whatever the connection runs next, so it runs
/// nothing more ([`Link::taint`]).
async fn idle(env: &Env<'_>, link: &mut Link) {
    if !env.killer.idle(env.busy).await {
        link.taint(DbError::CancelNotTaken(crate::session::kill_pending()));
    }
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
/// read to its end. Counts and checks that come meanwhile wait for it; when the session stops
/// meanwhile, they are put back in line, to be answered.
async fn hold(
    qr: &mut QueryResult<'_, 'static, TextProtocol>,
    link: &mut Link,
    env: &Env<'_>,
    id: u64,
    columns: &[Column],
    carry: Option<Vec<Cell>>,
    ended: bool,
) -> Result<Held, Stop> {
    let mut held = Held { stopped: false, failed: None, info: StatementInfo::default(), waiting: Vec::new() };
    let gate = Gate::new();
    let mut refusal = link.refusal(env.killer, &gate);
    let out = hold_on(qr, link, env, id, columns, carry, ended, &mut held, &mut refusal).await;
    drop(refusal);
    // Nothing sent again for this statement reaches the next one (see [`Link::guard`]).
    // An unclean gate after the result ended: the result is the run's all the same, and the
    // session is lost after it.
    if !gate.close().await && out.is_ok() {
        link.taint(DbError::CancelNotTaken(crate::session::kill_pending()));
    }
    match out {
        Ok(()) => Ok(held),
        Err(stop) => {
            link.requeue_all(std::mem::take(&mut held.waiting));
            Err(stop)
        }
    }
}

/// [`hold`], into `held`.
#[allow(clippy::too_many_arguments)]
async fn hold_on(
    qr: &mut QueryResult<'_, 'static, TextProtocol>,
    link: &mut Link,
    env: &Env<'_>,
    id: u64,
    columns: &[Column],
    mut carry: Option<Vec<Cell>>,
    mut ended: bool,
    held: &mut Held,
    refusal: &mut BoxFuture<'static, CancelFailure>,
) -> Result<(), Stop> {
    let page_size = env.page_size;
    let mut more = true;
    while more {
        // A cancel the server refused, or that was not sent, while the app had not asked for
        // the next page yet: cut as a request would be ([`Link::refusal`]).
        let next = tokio::select! {
            biased;
            next = link.next_command() => next,
            why = &mut *refusal => {
                link.cut();
                return Err(Stop::Lost(DbError::CancelNotTaken(why)));
            }
        };
        match next {
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
                            return Ok(());
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
                // Not read to its end in the background, whatever became of the kill.
                link.cut();
                return Err(Stop::Closed);
            }
            Next::Lost(reason) => return Err(Stop::Lost(reason)),
        }
    }
    held.stopped = !ended;
    if held.stopped {
        stop_result(qr, link, env).await?;
        return Ok(());
    }
    held.info = info_of(qr);
    match link.guard(Some(env.killer), rest(qr)).await? {
        Ok(sets) => held.info.more_results += sets,
        Err(e) => held.failed = Some(e),
    }
    Ok(())
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
    idle(env, link).await;
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
    s.routine |= held.info.more_results > 0;
    if let Some(error) = halt!(after(conn, link, env, s, sql, false, !ok || was_open, ok).await, None) {
        return Err(Halted { stop: Stop::Lost(error), reply: None });
    }
    link.requeue_all(held.waiting);
    Ok(())
}

/// Stop a result before its end: `KILL QUERY`, then read and drop what the server still sends
/// (its rows up to where it stopped, and the error that says so). A kill that does not take
/// cuts the connection instead ([`Link::guard`]).
async fn stop_result(
    qr: &mut QueryResult<'_, 'static, TextProtocol>,
    link: &mut Link,
    env: &Env<'_>,
) -> Result<(), Closed> {
    env.killer.clear();
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
/// A call of the function, not a name spelled so (a column `row_count`).
fn reads_outcome(sql: &str, mode: MySqlMode) -> bool {
    let toks: Vec<Token> = lex_in(sql, Dialect::MySql(mode)).into_iter().filter(|t| !t.is_trivia()).collect();
    toks.windows(2).any(|w| {
        w[0].is_word()
            && matches!(w[0].text(sql).to_ascii_uppercase().as_str(), "ROW_COUNT" | "FOUND_ROWS")
            && w[1].kind == Tok::LParen
    })
}

/// Whether `sql` reads with a locking clause (`FOR UPDATE`, `FOR SHARE`, `LOCK IN SHARE MODE`).
fn locking(sql: &str, mode: MySqlMode) -> bool {
    let w = words(sql, mode);
    w.windows(2).any(|p| p[0] == "FOR" && (p[1] == "UPDATE" || p[1] == "SHARE"))
        || w.windows(4).any(|p| p == ["LOCK", "IN", "SHARE", "MODE"])
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
    idle(env, link).await;
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
    idle(env, link).await;
    let snapshot = s.tx.open && matches!(isolation.as_str(), "REPEATABLE-READ" | "SERIALIZABLE");
    let result = match counted {
        Ok(Some(n)) => n.trim().parse::<u64>().map_err(|_| DbError::NoResult),
        Ok(None) => Err(DbError::NoResult),
        Err(e) if e.is_fatal() => return Err(Stop::Lost(my_error(&e))),
        Err(e) => {
            let ask = s.tx.open || !s.tx.autocommit;
            if let Some(error) = after(conn, link, env, s, "", false, ask, false).await? {
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

mod export;

#[cfg(test)]
mod tests;

/// Whether `sql` may run code of the server's that changes the session without the server saying
/// so (see [`State::routine`]): `runs_code` (a `CALL`, a stored function), an `EXECUTE` (the
/// prepared text is not read), or an executable comment (`/*! CALL p() */`, whose text the
/// classifier reads as a comment).
fn runs_server_code(sql: &str, mode: MySqlMode, runs_code: bool) -> bool {
    runs_code
        || words(sql, mode).first().is_some_and(|w| w == "EXECUTE")
        || lex_in(sql, Dialect::MySql(mode)).iter().any(|t| t.kind == Tok::ExecComment)
}

/// A statement that may have run code of the server's was stopped before the session could ask
/// what the code changed: its values are not known from now on (said to the app).
fn forget_after_code(env: &Env<'_>, s: &mut State) {
    if std::mem::take(&mut s.routine) {
        s.tracked.forget_values();
        s.values = s.tracked.values();
        if let Some(v) = s.values.clone() {
            let _ = env.events.send(DbEvent::SessionValues(v));
        }
    }
}
