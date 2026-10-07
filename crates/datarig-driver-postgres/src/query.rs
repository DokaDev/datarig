//! The *query* connection: user statements through a portal, paged with `Execute(max_rows)`,
//! and command tags for statements that return no rows.
//!
//! Round trips: the first page of a row-returning statement costs one round
//! trip when the statement is prepared already and two when it is not. `BEGIN` (outside the
//! user's block), Bind, Describe, Execute and Sync of the first page go out in one write
//! ([`Transaction::bind_first_page`]); prepared statements are kept per session by their text
//! ([`Prepared`]). By default (`PagingMode::NoHold`) the `COMMIT` that ends the portal's
//! transaction goes out in that write too ([`Transaction::bind_first_page_then`]): the first page
//! leaves nothing open on the server (no transaction, no lock, no snapshot), whether more rows
//! follow or not, and past it the app can only run the statement again (`Resume`); with
//! `PagingMode::Hold` the portal and its transaction stay open while more rows follow. A
//! statement the lexer knows returns no rows (DML without `RETURNING`, DDL, transaction control,
//! ...: [`returns_no_rows`]) is parsed, bound and executed in one write
//! ([`Client::execute_pipelined`]). Each later page of a held result is one round trip (Execute
//! + Sync).
//!
//! Transactions: after a successful transaction control statement (`BEGIN`, `COMMIT`, ...) the
//! session knows the block state from the statement itself ([`tx_after`]) and asks the server
//! nothing, so the user's transaction takes its snapshot at the user's first query and
//! `SET TRANSACTION` still works. It asks whether a block is open (one extra round trip,
//! [`probe`]) only where the statement cannot tell: after every failure, after `CALL` and `DO`,
//! and after text it cannot classify. It reports changes as `TxOpen`, and whether the block is
//! aborted (a statement in it failed; the probe tells) as `TxAborted`. Inside the user's block a
//! row-returning statement's portal lives in that block (no `BEGIN`/`COMMIT` of its own), so
//! reading never commits or ends the user's transaction.
//!
//! `EXPLAIN ANALYZE` runs its statement to measure it and keeps nothing:
//! outside the user's block the portal's own transaction is rolled back instead of committed;
//! inside it the statement runs in `SAVEPOINT datarig_explain`, rolled back to and released
//! afterwards, so the block keeps its earlier changes and is not aborted by a failure. What a
//! rollback cannot undo stays (a sequence's `nextval`, work outside the database).
//!
//! **Read-only sessions** (a read-only policy): every transaction the session
//! opens is `READ ONLY` on the server, which is what guarantees that nothing is written,
//! whatever the statement runs (a function, a trigger) and whatever the session's defaults say
//! (a pooler that dropped the startup option, a `set_config()` that turned it off):
//!
//! * a row-returning statement outside the user's block: its portal's transaction starts with
//!   `BEGIN READ ONLY` instead of `BEGIN` (same write, no extra round trip);
//! * a statement without rows outside the user's block: `BEGIN READ ONLY` before it and
//!   `COMMIT` after it, in the same write (a failure leaves the block aborted: it is rolled
//!   back, one round trip on that path). Transaction control and session settings are not
//!   wrapped: they write nothing, and some cannot run in a block (`DISCARD ALL`);
//! * a statement without rows that is not known to be one (the prepared path): its own
//!   `BEGIN READ ONLY` transaction, committed after it (one more round trip; rare);
//! * the user's own block: `SET TRANSACTION READ ONLY` goes out in the same write as the first
//!   statement after the `BEGIN` (the user's `BEGIN` is never rewritten). Setting read-only is
//!   allowed at any point of a transaction; going back to read-write is allowed only before its
//!   first query, so a statement that asks for read-write (`SET transaction_read_only = off`,
//!   `SET TRANSACTION READ WRITE`, `set_config()` of it, …) is never sent
//!   ([`DbError::ReadWriteRefused`]).
//!
//! **A search path per transaction**: a session in a schema of its own whose server
//! dropped the startup option (a pooler) sends `SET LOCAL search_path …` in every transaction it
//! opens or first uses, in the same write as what goes there anyway, exactly where a read-only
//! session sends `BEGIN READ ONLY` / `SET TRANSACTION READ ONLY` (the first statements of a
//! transaction, [`Env::begin`] and [`Env::first_in_block`]); a statement is also parsed in such
//! a transaction ([`Client::prepare_wrapped`]), because the server resolves names when it
//! parses. Nothing is ever set for the session: behind a transaction pooler it would stay on a
//! server connection other clients get.
//!
//! **The statement cache**: prepared row-returning statements are kept between
//! transactions ([`Prepared`]) unless the profile turned the cache off or the session turned it
//! off itself after the server lost them twice or had one of their names already
//! ([`DbEvent::StatementCacheOff`]), as behind a pooler in transaction mode without prepared
//! statement support. Off, no statement outlives the transaction it runs in.
//!
//! Closing: when the UI closes the session while a statement runs, the statement is cancelled
//! and the connection closed (the server rolls back). When the connection ends by itself, the
//! running statement fails first, then `Lost` is reported once.

use crate::connect::{db_error, is_cancel};
use crate::link::{Closed, Link, Next};
use crate::route::Cancel;
use crate::values::{Raw, format_code, format_value, is_json, is_numeric, type_display, value_kind};
use datarig_core::driver::{Cell, ColumnMeta, ColumnOrigin, DbCommand, DbError, DbEvent, Outcome, PagingMode};
use datarig_core::sql::lexer::{Tok, Token, lex};
use datarig_core::sql::risk::{self, Class};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Column, FirstPage, Row, SimpleQueryMessage, Statement, Transaction};

/// Whether a transaction is open, and what the UI was last told.
#[derive(Default)]
struct Tx {
    /// An explicit transaction block (the user's `BEGIN`), possibly aborted.
    block: bool,
    /// The block is aborted (a statement in it failed): only `ROLLBACK` works.
    aborted: bool,
    reported: bool,
    reported_aborted: bool,
    /// What the UI was last told of the user's block (`DbEvent::Block`).
    reported_block: bool,
    /// The last probe failed, so `block` may be out of date: the next run probes first.
    unsure: bool,
    /// The user opened a block on a session that sends statements first in each transaction
    /// (read-only, a search path per transaction): they go out with the next statement.
    first_pending: bool,
}

impl Tx {
    /// The block is open (or not) now, as a statement or a probe says; a block that was not
    /// open before gets the session's first statements ([`Env::first_in_block`]) when it has
    /// some (`due`).
    fn set_block(&mut self, open: bool, due: bool) {
        self.first_pending = due && open && (self.first_pending || !self.block);
        self.block = open;
    }

    /// `COMMIT AND CHAIN` / `ROLLBACK AND CHAIN` ended the block and opened a new one at once:
    /// the new one gets the session's first statements (`due`) as any new block does (a path
    /// set per transaction ended with the old one).
    fn chained(&mut self, due: bool) {
        self.first_pending = due;
        self.block = true;
    }

    /// The statements to run first in the user's block, if they are due (they are sent now).
    fn take_first(&mut self, env: &Env<'_>) -> Vec<String> {
        if std::mem::take(&mut self.first_pending) { env.first_in_block() } else { Vec::new() }
    }

    /// [`Tx::take_first`] as the start of a simple query (`…; `), or nothing.
    fn take_first_text(&mut self, env: &Env<'_>) -> String {
        self.take_first(env).iter().map(|f| format!("{f}; ")).collect()
    }
}

/// How a read-only session starts a transaction of its own.
const BEGIN_READ_ONLY: &str = "BEGIN READ ONLY";

/// What a read-only session runs first in the user's block.
const SET_READ_ONLY: &str = "SET TRANSACTION READ ONLY";

/// Whether `sql`, outside the user's block on a read-only session, runs in a `BEGIN READ ONLY`
/// transaction of its own: everything but transaction control and session settings.
fn guarded(sql: &str) -> bool {
    !matches!(risk::classify(sql).class, Class::Tx | Class::Session)
}

/// Whether `sql`, outside the user's block on a session with a search path per transaction,
/// runs in a transaction of its own that sets it: everything but transaction control and the
/// session settings that name nothing the path resolves. `PREPARE` is a session setting that
/// does (its statement is parsed now), so it goes in one; `DISCARD ALL` cannot run in a block.
fn path_guarded(sql: &str) -> bool {
    match risk::classify(sql).class {
        Class::Tx => false,
        Class::Session => first_word(sql).as_deref() == Some("PREPARE"),
        _ => true,
    }
}

/// The first word of `sql`, upper case.
fn first_word(sql: &str) -> Option<String> {
    lex(sql).into_iter().find(|t| !t.is_trivia()).filter(|t| t.is_word()).map(|t| t.text(sql).to_ascii_uppercase())
}

/// How the query session runs what it is sent.
pub(crate) struct Settings {
    pub(crate) page_size: usize,
    /// The session is read-only: every transaction it opens is `READ ONLY`.
    pub(crate) read_only: bool,
    /// The server dropped the search path's startup option: this goes first in each
    /// transaction (`SET LOCAL search_path …`).
    pub(crate) path: Option<String>,
    /// Keep prepared statements between transactions (the profile's statement cache).
    pub(crate) statement_cache: bool,
}

/// What a statement run needs besides the connection.
struct Env<'a> {
    events: &'a UnboundedSender<DbEvent>,
    token: &'a Cancel,
    /// The UI asked to cancel (set by the session's canceller, cleared when a run starts): a
    /// run of several statements stops before its next one.
    cancel: &'a AtomicBool,
    page_size: usize,
    /// The session is read-only: every transaction it opens is `READ ONLY`.
    read_only: bool,
    /// Sent first in every transaction (a search path per transaction), if anything.
    path: Option<&'a str>,
    /// The name of the savepoint a count or the allowlist's question runs under inside a
    /// transaction ([`count_savepoint`]): never one of the user's own.
    savepoint: &'a str,
    /// Tests: runs right before each row-returning statement is bound (e.g. DDL on another
    /// connection, to make its prepared statement stale at will).
    #[cfg(test)]
    before_bind: Option<&'a BeforeBind>,
}

#[cfg(test)]
type BeforeBind = dyn Fn() -> futures::future::BoxFuture<'static, ()> + Sync;

impl Env<'_> {
    /// The session sends statements first in each transaction.
    fn wraps(&self) -> bool {
        self.read_only || self.path.is_some()
    }

    /// `sql`, outside the user's block, runs in a transaction of its own that starts with the
    /// session's first statements ([`Env::begin`]): [`guarded`] on a read-only session,
    /// [`path_guarded`] with a search path per transaction.
    fn wrapped(&self, sql: &str) -> bool {
        (self.read_only && guarded(sql)) || (self.path.is_some() && path_guarded(sql))
    }

    /// How a transaction of the session's own starts: `BEGIN` (`BEGIN READ ONLY`), then the
    /// search path when it is set per transaction.
    fn begin(&self) -> Vec<String> {
        let begin = if self.read_only { BEGIN_READ_ONLY } else { "BEGIN" };
        std::iter::once(begin).chain(self.path).map(str::to_string).collect()
    }

    /// What goes first in the user's new block: `SET TRANSACTION READ ONLY`, the search path.
    fn first_in_block(&self) -> Vec<String> {
        self.read_only.then_some(SET_READ_ONLY).into_iter().chain(self.path).map(str::to_string).collect()
    }

    /// The search path set for a simple query of its own (one implicit transaction), as its
    /// start (`…; `), or nothing.
    fn path_text(&self) -> String {
        self.path.map(|p| format!("{p}; ")).unwrap_or_default()
    }

    fn report(&self, tx: &mut Tx, open: bool) {
        // The user's block first: the UI learns how it ended (a rollback, an aborted block
        // committed) before the transaction reads as closed.
        if tx.reported_block != tx.block {
            tx.reported_block = tx.block;
            let _ = self.events.send(DbEvent::Block(tx.block));
        }
        if tx.reported != open {
            tx.reported = open;
            let _ = self.events.send(DbEvent::TxOpen(open));
        }
        let aborted = tx.block && tx.aborted;
        if tx.reported_aborted != aborted {
            tx.reported_aborted = aborted;
            let _ = self.events.send(DbEvent::TxAborted(aborted));
        }
    }
}

/// The answer to one run (`Execute`). **Every run ends with exactly one terminal event**, on
/// every path: its first page (`Page` with columns), `Done`, or `Failed` (a cancel is a
/// `Failed` with `cancelled`). The UI ends the run at that event and waits for nothing else,
/// so a run that ended without one would leave its tab running for good.
///
/// * Every terminal event of the run goes through its `Reply`, and each answer consumes it
///   (`page`, `done`, `fail`, `closed` take `self`): a second terminal event does not compile.
///   After a first page the run holds a [`Paged`], whose one `Failed` may still replace the
///   rows shown (a failed `COMMIT`, a failed fetch of a later page, rows a rule returned beyond
///   the page). Later pages (`Page` without columns) answer `FetchMore` requests and are not
///   terminal.
/// * A `Reply` dropped unanswered (a path that forgot, a panic) answers with
///   [`DbError::NoResult`] and trips a debug assertion; the run of a session the UI closed
///   ([`Reply::closed`]) answers nothing, as the UI expects.
struct Reply<'a> {
    events: &'a UnboundedSender<DbEvent>,
    id: u64,
    /// An answer was given (or the UI closed the session): the drop guard stays quiet.
    answered: bool,
}

impl<'a> Reply<'a> {
    fn new(events: &'a UnboundedSender<DbEvent>, id: u64) -> Self {
        Self { events, id, answered: false }
    }

    /// The first page of the result; one failure may still follow ([`Paged::fail`]).
    fn page(
        mut self,
        columns: Vec<ColumnMeta>,
        rows: Vec<Vec<Cell>>,
        more: bool,
        elapsed: std::time::Duration,
    ) -> Paged<'a> {
        self.answered = true;
        let _ = self.events.send(DbEvent::Page { id: self.id, columns: Some(columns), rows, more, elapsed });
        Paged { events: self.events, id: self.id }
    }

    fn done(mut self, outcome: Outcome, elapsed: std::time::Duration) {
        self.answered = true;
        let _ = self.events.send(DbEvent::Done { id: self.id, outcome, elapsed });
    }

    fn fail(mut self, error: DbError, cancelled: bool) {
        self.answered = true;
        let _ = self.events.send(DbEvent::Failed { id: self.id, error, cancelled });
    }

    fn fail_with(self, e: &tokio_postgres::Error) {
        self.fail(db_error(e), is_cancel(e));
    }

    /// [`Reply::fail_with`] for a statement that ran in a transaction of the session's own
    /// (`path_tx`: one that sets the search path, see [`failure`]) or not.
    fn fail_in(self, e: &tokio_postgres::Error, path_tx: bool) {
        let (error, cancelled) = failure(e, path_tx);
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

/// A failure of a statement as the UI hears it (and whether it was a cancel). `path_tx`: it ran
/// in a transaction the session opened only to set its search path: a statement
/// that cannot run in a transaction, or a procedure that commits, failed only for that, which the
/// UI says ([`DbError::NeedsNoTransaction`]).
fn failure(e: &tokio_postgres::Error, path_tx: bool) -> (DbError, bool) {
    let code = e.code();
    let blocked =
        code == Some(&SqlState::ACTIVE_SQL_TRANSACTION) || code == Some(&SqlState::INVALID_TRANSACTION_TERMINATION);
    match db_error(e) {
        DbError::Server(msg) if path_tx && blocked => (DbError::NeedsNoTransaction(msg), false),
        error => (error, is_cancel(e)),
    }
}

/// A run answered with its first page: one `Failed` may still replace the rows, and nothing
/// else. Dropping it sends nothing (the page was the answer).
struct Paged<'a> {
    events: &'a UnboundedSender<DbEvent>,
    id: u64,
}

impl Paged<'_> {
    fn fail(self, error: DbError, cancelled: bool) {
        let _ = self.events.send(DbEvent::Failed { id: self.id, error, cancelled });
    }

    fn fail_with(self, e: &tokio_postgres::Error) {
        self.fail(db_error(e), is_cancel(e));
    }
}

/// Why a statement run stopped the session.
enum Stop {
    Closed,
    Lost(datarig_core::driver::DbError),
}

impl From<Closed> for Stop {
    fn from(_: Closed) -> Self {
        Stop::Closed
    }
}

/// A run stopped by the session ([`Stop`]), with its reply when it was not answered yet: the
/// run's caller answers it as the stop requires (nothing for a closed session, the failure for
/// a lost connection).
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

pub(crate) async fn query_loop(
    mut client: Client,
    mut link: Link,
    token: Cancel,
    cancel: Arc<AtomicBool>,
    events: UnboundedSender<DbEvent>,
    settings: Settings,
) {
    let env = Env {
        events: &events,
        token: &token,
        cancel: &cancel,
        page_size: settings.page_size,
        read_only: settings.read_only,
        path: settings.path.as_deref(),
        savepoint: count_savepoint(),
        #[cfg(test)]
        before_bind: None,
    };
    let mut tx = Tx::default();
    let mut prepared = Prepared { off: !settings.statement_cache, ..Prepared::default() };
    loop {
        let stop = match link.next().await {
            Next::Command(DbCommand::Execute { id, statements, paging }) => {
                let mut s = State { tx: &mut tx, prepared: &mut prepared };
                match execute(&mut client, &mut link, &env, &mut s, id, statements, None, paging).await {
                    Ok(()) => continue,
                    Err(stop) => stop,
                }
            }
            // Run again past `skip` rows: an `Execute` of one statement that drops them first.
            Next::Command(DbCommand::Resume { id, sql, skip, paging }) => {
                let mut s = State { tx: &mut tx, prepared: &mut prepared };
                match execute(&mut client, &mut link, &env, &mut s, id, vec![sql], Some(skip), paging).await {
                    Ok(()) => continue,
                    Err(stop) => stop,
                }
            }
            // Answered once; a session the UI closed meanwhile answers nothing.
            Next::Command(DbCommand::Count { id, sql }) => match count(&client, &mut link, &env, &mut tx, &sql).await {
                Ok((result, snapshot)) => {
                    let _ = events.send(DbEvent::Counted { id, result, snapshot });
                    continue;
                }
                Err(Closed) => Stop::Closed,
            },
            Next::Command(DbCommand::CheckRepeat { id, sql }) => {
                match check_repeat(&client, &mut link, &env, &mut tx, &sql).await {
                    Ok(result) => {
                        let _ = events.send(DbEvent::RepeatChecked { id, result });
                        continue;
                    }
                    Err(Closed) => Stop::Closed,
                }
            }
            // A fetch for a result whose portal is gone (complete, failed or closed): answered
            // with an empty last page, so the UI never waits for it.
            Next::Command(DbCommand::FetchMore { id }) => {
                let _ = events.send(DbEvent::Page {
                    id,
                    columns: None,
                    rows: Vec::new(),
                    more: false,
                    elapsed: std::time::Duration::ZERO,
                });
                continue;
            }
            Next::Command(_) => continue,
            Next::Closed => Stop::Closed,
            Next::Lost(reason) => Stop::Lost(reason),
        };
        if let Stop::Lost(error) = stop {
            let _ = events.send(DbEvent::Lost { error });
        }
        return;
    }
}

/// Ask the server whether a transaction block is open (an aborted block answers with
/// `25P02`) and report a change.
async fn probe(client: &Client, link: &mut Link, env: &Env<'_>, tx: &mut Tx) -> Result<(), Closed> {
    // The simple protocol runs it as one message: outside a block its implicit transaction
    // starts with this very statement, so both timestamps are equal. (With the extended
    // protocol the timestamps of Parse and Execute differ.)
    let sql = "SELECT pg_catalog.now() <> pg_catalog.statement_timestamp()";
    let block;
    (block, tx.aborted, tx.unsure) = match link.guard(None, client.simple_query(sql)).await? {
        Ok(msgs) => {
            (msgs.iter().any(|m| matches!(m, SimpleQueryMessage::Row(r) if r.get(0) == Some("t"))), false, false)
        }
        Err(e) if e.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION) => (true, true, false),
        Err(_) => (tx.block, tx.aborted, true),
    };
    tx.set_block(block, env.wraps());
    env.report(tx, tx.block);
    Ok(())
}

/// End the transaction that held a portal: commit or roll back the one of its own (a handle on
/// the user's block ends nothing). A failed `COMMIT` comes back.
async fn finish(
    txn: Transaction<'_>,
    commit: bool,
    link: &mut Link,
) -> Result<Result<(), tokio_postgres::Error>, Closed> {
    if commit { link.guard(None, txn.commit()).await } else { link.guard(None, txn.rollback()).await }
}

/// Prepared row-returning statements of the session by their text, the most recently used
/// last. A statement run again outside the user's block skips its Parse round trip (inside it
/// every statement is prepared again, and not kept). Dropping one closes it on the server
/// (sent with the next request).
///
/// **Off** (the profile's statement cache is off, or the session turned it off: [`turn_off`]),
/// no statement outlives the transaction it runs in, for a pooler in transaction mode without
/// prepared statement support, where the next transaction may run on another server
/// connection: outside the user's block a statement is prepared as the unnamed statement right
/// after its transaction's `BEGIN` ([`prepare_opened`]) and bound in that transaction; inside
/// the block it is prepared there and closed with it; the statements of type lookups are
/// dropped before the transaction ends. Each statement then costs what a new one costs with
/// the cache on (two round trips for a first page instead of one).
#[derive(Default)]
struct Prepared {
    by_text: HashMap<String, Statement>,
    order: VecDeque<String>,
    off: bool,
    /// How often the server lost the session's prepared statements (`26000`, [`gone`]). Once
    /// can be the user's own doing (`DISCARD ALL` in a function); twice turns the cache off.
    lost: u8,
}

/// How many prepared statements a session keeps.
const PREPARED_KEPT: usize = 64;

impl Prepared {
    fn get(&mut self, sql: &str) -> Option<Statement> {
        let stmt = self.by_text.get(sql)?.clone();
        self.touch(sql);
        Some(stmt)
    }

    fn put(&mut self, sql: &str, stmt: Statement) {
        if self.by_text.insert(sql.to_string(), stmt).is_none() {
            self.order.push_back(sql.to_string());
        } else {
            self.touch(sql);
        }
        while self.order.len() > PREPARED_KEPT {
            if let Some(old) = self.order.pop_front() {
                self.by_text.remove(&old);
            }
        }
    }

    /// Forget them all (the server no longer has them).
    fn clear(&mut self) {
        self.by_text.clear();
        self.order.clear();
    }

    fn forget(&mut self, sql: &str) {
        if self.by_text.remove(sql).is_some() {
            self.order.retain(|s| s != sql);
        }
    }

    fn touch(&mut self, sql: &str) {
        if let Some(i) = self.order.iter().position(|s| s == sql) {
            let s = self.order.remove(i).unwrap_or_default();
            self.order.push_back(s);
        }
    }
}

/// A prepared statement the server refuses to run because its result changed since it was
/// prepared (DDL, a different `search_path`): "cached plan must not change result type",
/// told apart by the server function that raises it (the message is translated). Or one the
/// server no longer has ([`gone`]).
fn stale(e: &tokio_postgres::Error) -> bool {
    gone(e)
        || e.code() == Some(&SqlState::FEATURE_NOT_SUPPORTED)
            && e.as_db_error().and_then(|d| d.routine()) == Some("RevalidateCachedQuery")
}

/// The server no longer has the prepared statement (`26000`, invalid_sql_statement_name):
/// something the session did not see coming deallocated it (`DEALLOCATE ALL` or `DISCARD ALL`
/// inside a function or a `DO` block, a pooler's reset), most likely with all the others.
fn gone(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::INVALID_SQL_STATEMENT_NAME)
}

/// A prepared statement of the session with that name exists already (`42P05`,
/// duplicate_prepared_statement): one that another client left on a pooled server connection.
fn duplicate(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::DUPLICATE_PSTATEMENT)
}

/// The server lost a prepared statement of the session (`gone`) or had one of its name already
/// (`duplicate`): count it, and turn the cache off (said once) when that is not the first time
/// or was a duplicate. Whether the cache is off now.
fn vanished(prepared: &mut Prepared, env: &Env<'_>, duplicate: bool) -> bool {
    if prepared.off {
        return true;
    }
    prepared.lost = prepared.lost.saturating_add(1);
    if prepared.lost >= 2 || duplicate {
        turn_off(prepared, env);
    }
    prepared.off
}

/// Stop keeping prepared statements for the rest of the session ([`Prepared`]), and say so.
fn turn_off(prepared: &mut Prepared, env: &Env<'_>) {
    prepared.clear();
    prepared.off = true;
    let _ = env.events.send(DbEvent::StatementCacheOff);
}

/// Whether `sql` deallocates the session's prepared statements, from its text: `DEALLOCATE`
/// (one or all: the name may be one of ours), `DISCARD ALL` and `DISCARD PLANS`. The session
/// then forgets all of them before it runs, instead of finding out from a failure.
pub(crate) fn forgets_prepared(sql: &str) -> bool {
    let words: Vec<String> = lex(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .take(2)
        .map(|t| if t.is_word() { t.text(sql).to_ascii_uppercase() } else { String::new() })
        .collect();
    match words.first().map(String::as_str) {
        Some("DEALLOCATE") => true,
        Some("DISCARD") => matches!(words.get(1).map(String::as_str), Some("ALL" | "PLANS")),
        _ => false,
    }
}

/// The session's state a run changes.
struct State<'a> {
    tx: &'a mut Tx,
    prepared: &'a mut Prepared,
}

/// The result's columns. The RowDescription names the table (`oid`) and column number of a
/// column that comes straight from a table; an expression has neither (0), so no origin.
fn columns_of(columns: &[Column]) -> Vec<ColumnMeta> {
    columns
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_string(),
            type_name: type_display(c.type_()),
            numeric: is_numeric(c.type_()),
            json: is_json(c.type_()),
            kind: value_kind(c.type_()),
            origin: match (c.table_oid(), c.column_id()) {
                (Some(table), Some(column)) if table != 0 && column > 0 => Some(ColumnOrigin::Pg { table, column }),
                _ => None,
            },
        })
        .collect()
}

/// The result format of each column of `stmt` ([`format_code`]).
fn formats_of(stmt: &Statement) -> Vec<i16> {
    stmt.columns().iter().map(|c| format_code(c.type_())).collect()
}

/// The display text of every value of `rows`, fetched in `formats`.
fn decode(rows: &[Row], formats: &[i16]) -> Vec<Vec<Cell>> {
    rows.iter()
        .map(|row| {
            (0..row.len())
                .map(|i| {
                    let ty = row.columns()[i].type_();
                    let text = formats.get(i) == Some(&0);
                    match row.try_get::<_, Raw>(i) {
                        Ok(Raw(Some(b))) => Some(format_value(ty, b, text)),
                        Ok(Raw(None)) => None,
                        Err(e) => Some(e.to_string()),
                    }
                })
                .collect()
        })
        .collect()
}

/// PostgreSQL does not expose the CommandComplete tag through tokio-postgres, so derive
/// it from the statement text the same way the server names it (e.g. `CREATE TABLE`).
pub(crate) fn command_tag(sql: &str) -> (String, bool) {
    let words: Vec<String> =
        lex(sql).iter().filter(|t| t.is_word()).map(|t| t.text(sql).to_ascii_uppercase()).collect();
    let Some(first) = words.first() else { return (String::new(), false) };
    let dml = |w: &str| matches!(w, "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "COPY");
    if dml(first) {
        return (first.clone(), true);
    }
    if first == "WITH"
        && let Some(w) = words.iter().find(|w| dml(w))
    {
        return (w.clone(), true);
    }
    if matches!(first.as_str(), "CREATE" | "DROP" | "ALTER") {
        let skip = ["OR", "REPLACE", "TEMP", "TEMPORARY", "UNIQUE", "UNLOGGED", "IF", "NOT", "EXISTS"];
        if let Some(obj) = words[1..].iter().find(|w| !skip.contains(&w.as_str())) {
            if obj == "MATERIALIZED" {
                return (format!("{first} MATERIALIZED VIEW"), false);
            }
            return (format!("{first} {obj}"), false);
        }
    }
    (first.clone(), false)
}

/// What a statement without rows that succeeded did to the transaction state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TxAfter {
    /// Nothing: the state is what it was.
    Same,
    /// Known from the statement: a block is open (`true`, the same one, or a new one after
    /// idle) or not.
    Block(bool),
    /// `COMMIT AND CHAIN` / `ROLLBACK AND CHAIN`: the block ended and a new one is open.
    Chain,
    /// Unknown: the session has to ask the server ([`probe`]).
    Ask,
}

/// The transaction state after `sql` succeeded. Failures are not classified: after a failure
/// the session always asks.
///
/// Transaction control sets the state without asking. Asking would run a query in the new
/// transaction: it would take the snapshot (at `BEGIN` instead of at the user's first query,
/// for `REPEATABLE READ`) and make a following `SET TRANSACTION` fail ("must be called before
/// any query"). What a successful statement leaves behind, checked against PostgreSQL 17:
///
/// | statement                            | before               | after                      |
/// |--------------------------------------|----------------------|----------------------------|
/// | `BEGIN`, `START TRANSACTION`         | idle                 | block                      |
/// |                                      | block (warning)      | the same block             |
/// | `COMMIT`, `END`, `ROLLBACK`, `ABORT` | block                | idle                       |
/// |                                      | aborted block        | idle (`COMMIT` rolls back) |
/// |                                      | idle (warning)       | idle                       |
/// | the same `AND CHAIN`                 | block, aborted block | a new block                |
/// |                                      | idle                 | fails                      |
/// | `SAVEPOINT`, `RELEASE`               | block                | block                      |
/// |                                      | idle                 | fails                      |
/// | `ROLLBACK [WORK] TO`                 | block, aborted block | block, repaired            |
/// |                                      | idle                 | fails                      |
/// | `PREPARE TRANSACTION`                | block                | idle (prepared)*           |
/// |                                      | aborted block, idle  | idle (rolled back)         |
/// | `COMMIT`/`ROLLBACK PREPARED`         | idle                 | idle                       |
/// |                                      | block                | fails                      |
///
/// \* Documented: the test server has prepared transactions disabled, so there it fails.
///
/// An aborted block is a block to the indicator; any other statement fails there, so a success
/// never leaves one behind. A chain is [`TxAfter::Chain`]: the new block starts without what the
/// session set first in the old one.
///
/// [`TxAfter::Ask`] for `CALL` and `DO` (a procedure may `COMMIT` and fail after it), and
/// whenever the text cannot be classified: no leading word, or more than one statement.
///
/// [`TxAfter::Same`] for everything else (DML, DDL, `SELECT`, `SET`, ...). In PostgreSQL only
/// transaction control moves between idle and a block, and only an error aborts a block;
/// functions, triggers and rules cannot run transaction control. Each request of the extended
/// protocol ends with `Sync`, which ends an implicit transaction, so nothing is left open
/// outside a block. DDL inside a block stays in it (and one that refuses to run in a block
/// fails). `SET TRANSACTION` changes only the characteristics of the current transaction (and
/// must come before its first query); outside a block it only warns. A `SET` inside a block is
/// undone by a later `ROLLBACK`, but does not change the block itself.
///
/// Only statements without rows are classified: a row-returning statement runs in the user's
/// block or in a transaction of its own portal, where transaction control (also inside a
/// procedure) fails, so its success never changes the state.
pub(crate) fn tx_after(sql: &str) -> TxAfter {
    let toks: Vec<Token> = lex(sql).into_iter().filter(|t| !t.is_trivia()).collect();
    if let Some(semi) = toks.iter().position(|t| t.kind == Tok::Semi)
        && toks[semi..].iter().any(|t| t.kind != Tok::Semi)
    {
        return TxAfter::Ask;
    }
    let word = |i: usize| toks.get(i).filter(|t| t.is_word()).map(|t| t.text(sql).to_ascii_uppercase());
    let is = |i: usize, w: &str| word(i).as_deref() == Some(w);
    let Some(first) = word(0) else { return TxAfter::Ask };
    match first.as_str() {
        "BEGIN" | "START" | "SAVEPOINT" | "RELEASE" => TxAfter::Block(true),
        "COMMIT" | "ROLLBACK" if is(1, "PREPARED") => TxAfter::Block(false),
        "COMMIT" | "END" | "ROLLBACK" | "ABORT" => {
            let to = (1..=2).any(|i| is(i, "TO"));
            let chain = (1..toks.len()).any(|i| is(i, "AND") && is(i + 1, "CHAIN"));
            if chain && !to { TxAfter::Chain } else { TxAfter::Block(to) }
        }
        "PREPARE" if is(1, "TRANSACTION") => TxAfter::Block(false),
        "CALL" | "DO" => TxAfter::Ask,
        _ => TxAfter::Same,
    }
}

/// Whether `sql` is known to return no rows, from its text: DML without `RETURNING` and
/// statements that never return rows (DDL, transaction control, `SET`, `DO`, ...). Such a
/// statement is parsed, bound and executed in one round trip, without asking first whether it
/// has rows. One statement only; anything else (`SELECT`, `CALL`, `EXPLAIN`, `SHOW`, `COPY`,
/// text it cannot classify) takes the prepared path. A rule can still make DML return rows:
/// [`execute`] handles that.
pub(crate) fn returns_no_rows(sql: &str) -> bool {
    let toks: Vec<Token> = lex(sql).into_iter().filter(|t| !t.is_trivia()).collect();
    if let Some(semi) = toks.iter().position(|t| t.kind == Tok::Semi)
        && toks[semi..].iter().any(|t| t.kind != Tok::Semi)
    {
        return false;
    }
    let word = |t: &Token| t.is_word().then(|| t.text(sql).to_ascii_uppercase());
    let Some(first) = toks.first().and_then(word) else { return false };
    match first.as_str() {
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" => !toks.iter().any(|t| word(t).as_deref() == Some("RETURNING")),
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "COMMENT" | "SET" | "RESET" | "BEGIN"
        | "START" | "COMMIT" | "END" | "ROLLBACK" | "ABORT" | "SAVEPOINT" | "RELEASE" | "LOCK" | "DISCARD" | "DO"
        | "REFRESH" | "VACUUM" | "ANALYZE" | "REINDEX" | "CLUSTER" | "CHECKPOINT" | "LISTEN" | "UNLISTEN"
        | "NOTIFY" => true,
        "PREPARE" => toks.get(1).and_then(word).as_deref() == Some("TRANSACTION"),
        _ => false,
    }
}

/// Whether the rows of `sql` can be shown before its transaction commits: a plain read
/// (`SELECT`, `VALUES`, `TABLE`, `SHOW`). The rows of anything that may write
/// (`INSERT ... RETURNING`, `WITH`, ...) are shown only once the `COMMIT` succeeded.
fn plain_read(sql: &str) -> bool {
    let first = lex(sql).into_iter().find(|t| !t.is_trivia()).filter(|t| t.is_word());
    first.is_some_and(|t| matches!(t.text(sql).to_ascii_uppercase().as_str(), "SELECT" | "VALUES" | "TABLE" | "SHOW"))
}

/// What a statement without rows that succeeded leaves: its outcome (the last one of a run
/// answers with it) and the transaction state. The reply comes back while the run goes on.
#[allow(clippy::too_many_arguments)]
async fn no_rows_done<'a>(
    client: &Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    reply: Reply<'a>,
    sql: &str,
    step: Step,
    count: u64,
) -> Result<Option<(Reply<'a>, Outcome)>, Halted<'a>> {
    let (tag, is_dml) = command_tag(sql);
    let outcome = if is_dml { Outcome::Affected(count) } else { Outcome::Command(tag) };
    let reply = if step.last {
        reply.done(outcome, step.start.elapsed());
        None
    } else {
        Some((reply, outcome))
    };
    match tx_after(sql) {
        TxAfter::Same => {}
        // Transaction control that succeeded leaves no aborted block (`ROLLBACK TO` repairs one).
        TxAfter::Block(open) => {
            tx.set_block(open, env.wraps());
            (tx.aborted, tx.unsure) = (false, false);
            env.report(tx, open);
        }
        TxAfter::Chain => {
            tx.chained(env.wraps());
            (tx.aborted, tx.unsure) = (false, false);
            env.report(tx, true);
        }
        TxAfter::Ask => halt!(probe(client, link, env, tx).await, reply.map(|(r, _)| r)),
    }
    Ok(reply)
}

/// Run `statements`; for the last row-returning one keep the portal open and serve
/// `FetchMore` requests (`PagingMode::Hold`, or inside the user's block), or end it with its
/// first page (`PagingMode::NoHold`). A command that ends the open portal is processed next.
/// The run ends with exactly one terminal event ([`Reply`]).
#[allow(clippy::too_many_arguments)]
async fn execute(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State<'_>,
    id: u64,
    statements: Vec<String>,
    resume: Option<u64>,
    paging: PagingMode,
) -> Result<(), Stop> {
    let reply = Reply::new(env.events, id);
    match run(client, link, env, s, reply, statements, resume, paging).await {
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

#[allow(clippy::too_many_arguments)]
async fn run<'a>(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'a>,
    s: &mut State<'_>,
    reply: Reply<'a>,
    statements: Vec<String>,
    resume: Option<u64>,
    paging: PagingMode,
) -> Result<(), Halted<'a>> {
    let n = statements.len();
    if s.tx.unsure {
        halt!(probe(client, link, env, s.tx).await, Some(reply));
    }
    if n == 0 {
        reply.done(Outcome::Command(String::new()), std::time::Duration::ZERO);
        return Ok(());
    }
    // A cancel asked for before this run belonged to the one before.
    env.cancel.store(false, Ordering::SeqCst);
    // A statement run again for the user (`Resume`): what only the server can tell for the
    // allowlist is asked first, and inside the user's block the savepoint it runs under is set
    // in the same request (made read-only before it when that is due, so a rollback to the
    // savepoint keeps that).
    if resume.is_some() {
        let check = match check_text(&statements[0]) {
            Ok(check) => check,
            Err(why) => {
                reply.fail(why, false);
                return Ok(());
            }
        };
        let first = s.tx.block.then(|| format!("{}{RESUME_SAVEPOINT}", s.tx.take_first_text(env)));
        let text = first.as_ref().map_or_else(|| format!("{}{check}", env.path_text()), |f| format!("{f}; {check}"));
        let asked = halt!(link.guard(Some(env.token), client.simple_query(&text)).await, Some(reply));
        let refused = match asked {
            Ok(msgs) => check_answer(&msgs).err(),
            Err(e) => Some(if is_cancel(&e) { DbError::Cancelled } else { db_error(&e) }),
        };
        if let Some(why) = refused {
            if first.is_some() {
                // An aborted block refuses this too, and stays as it was.
                let _ = halt!(link.guard(None, client.batch_execute(RESUME_UNDO)).await, Some(reply));
            }
            let cancelled = why == DbError::Cancelled;
            reply.fail(why, cancelled);
            halt!(probe(client, link, env, s.tx).await, None);
            return Ok(());
        }
    }
    let mut reply = reply;
    for (i, sql) in statements.iter().enumerate() {
        let (skip, resume) = (resume.unwrap_or(0), resume.is_some());
        let hold = paging == PagingMode::Hold;
        let step = Step { last: i + 1 == n, index: i, start: Instant::now(), skip, resume, hold };
        if n > 1 {
            // Cancelled between two statements: the rest does not run.
            if i > 0 && env.cancel.load(Ordering::SeqCst) {
                reply.fail(DbError::Cancelled, true);
                return Ok(());
            }
            let _ = env.events.send(DbEvent::Started { id: reply.id, index: i });
        }
        // `EXPLAIN ANALYZE` inside the user's block: a savepoint keeps the block as it was.
        let savepoint = s.tx.block && rolls_back(sql);
        if savepoint {
            // A read-only block is made so before the savepoint: set inside it, a rollback to
            // the savepoint would undo it.
            let set = format!("{}SAVEPOINT {SAVEPOINT}", s.tx.take_first_text(env));
            if let Err(e) = halt!(link.guard(Some(env.token), client.batch_execute(&set)).await, Some(reply)) {
                // An aborted block refuses it, as it would refuse the statement.
                reply.fail_with(&e);
                halt!(probe(client, link, env, s.tx).await, None);
                return Ok(());
            }
        }
        let answer = statement(client, link, env, s, reply, sql, step).await?;
        if savepoint {
            let undo = format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}; RELEASE SAVEPOINT {SAVEPOINT}");
            let undone = halt!(link.guard(None, client.batch_execute(&undo)).await, answer.map(|(r, _)| r));
            if let Err(e) = undone {
                // Only a broken connection gets here; the failure aborts the block, so a later
                // COMMIT cannot keep the statement's changes.
                if let Some((reply, _)) = answer {
                    reply.fail_with(&e);
                }
                halt!(probe(client, link, env, s.tx).await, None);
                return Ok(());
            }
            // Rolled back to before the statement: a failure of it no longer aborts the block.
            s.tx.aborted = false;
            env.report(s.tx, s.tx.block);
        }
        reply = match answer {
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

/// Run one statement of a run. The reply comes back with what the statement did when the run
/// goes on (a statement before the last that succeeded); `None` once the run was answered (its
/// last statement, or a failure).
async fn statement<'a>(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'a>,
    s: &mut State<'_>,
    reply: Reply<'a>,
    sql: &str,
    step: Step,
) -> Result<Option<(Reply<'a>, Outcome)>, Halted<'a>> {
    let token = Some(env.token);
    // A read-only session never asks for read-write: the user's block could go back to
    // read-write before its first query, whatever `SET TRANSACTION READ ONLY` said.
    if env.read_only && risk::classify(sql).read_write {
        reply.fail(DbError::ReadWriteRefused, false);
        return Ok(None);
    }
    if forgets_prepared(sql) {
        s.prepared.clear();
        // The client's own statements for type lookups go with them.
        client.forget_typeinfo_statements();
    }
    if returns_no_rows(sql) {
        return no_rows(client, link, env, s.tx, reply, sql, step).await;
    }

    // The prepared statement of this text, prepared again inside the user's block: a
    // statement whose result changed since it was prepared fails at Bind, and a failure
    // there would abort the user's transaction.
    let mut prepared_again = false;
    // Run once more after the cache went off because the server lost a statement ([`vanished`]).
    let mut fell_back = false;
    let mut reply = reply;
    loop {
        // With the cache off, outside the user's block: prepared right after its transaction's
        // `BEGIN`, which stays open for it ([`prepare_opened`]).
        let opened = s.prepared.off && !s.tx.block;
        let kept = if s.tx.block || s.prepared.off { None } else { s.prepared.get(sql) };
        let reused = kept.is_some();
        let stmt = match kept {
            Some(stmt) => stmt,
            None => {
                let prepared = if opened {
                    halt!(link.guard(token, prepare_opened(client, env, sql)).await, Some(reply))
                } else {
                    halt!(link.guard(token, prepare(client, env, s.tx, sql, s.prepared.off)).await, Some(reply))
                };
                if s.prepared.off {
                    // A type lookup's statements go with the transaction too.
                    client.forget_typeinfo_statements();
                }
                match prepared {
                    Ok(stmt) => stmt,
                    Err(e) => {
                        if opened || env.path.is_some() && !s.tx.block {
                            // The failure left the transaction it was parsed in open and aborted.
                            let _ = halt!(link.guard(None, client.batch_execute("ROLLBACK")).await, Some(reply));
                        }
                        // A statement of a type lookup the server lost, or one of the same
                        // name another client left: nothing ran, so outside the user's block
                        // it is prepared once more (without the cache when it went off).
                        if gone(&e) || duplicate(&e) {
                            client.forget_typeinfo_statements();
                            let off = vanished(s.prepared, env, duplicate(&e));
                            if !s.tx.block && off && !fell_back {
                                fell_back = true;
                                continue;
                            }
                            if !s.tx.block && !off && !prepared_again {
                                prepared_again = true;
                                continue;
                            }
                        }
                        if step.resume && s.tx.block {
                            let _ = halt!(link.guard(None, client.batch_execute(RESUME_UNDO)).await, Some(reply));
                        }
                        reply.fail_with(&e);
                        halt!(probe(client, link, env, s.tx).await, None);
                        return Ok(None);
                    }
                }
            }
        };
        if opened && stmt.columns().is_empty() {
            // Its transaction was opened only to parse it: it runs as statements without rows
            // do (some cannot run in a transaction block), one round trip more.
            drop(stmt);
            let _ = halt!(link.guard(None, client.batch_execute("ROLLBACK")).await, Some(reply));
            return no_rows(client, link, env, s.tx, reply, sql, step).await;
        }
        if stmt.columns().is_empty() && (s.tx.first_pending || !s.tx.block && env.wrapped(sql)) {
            return wrapped_no_rows(client, link, env, s.tx, reply, &stmt, sql, step).await;
        }
        if stmt.columns().is_empty() {
            return match halt!(link.guard(token, client.execute(&stmt, &[])).await, Some(reply)) {
                Ok(count) => no_rows_done(client, link, env, s.tx, reply, sql, step, count).await,
                // Gone before it ran (outside the user's block, its Bind is a request of its
                // own): prepared and run once more.
                Err(e) if gone(&e) && !s.tx.block && (!prepared_again || !fell_back) => {
                    let off = vanished(s.prepared, env, false);
                    if off && !fell_back {
                        fell_back = true;
                    } else if !prepared_again {
                        prepared_again = true;
                    } else {
                        reply.fail_with(&e);
                        halt!(probe(client, link, env, s.tx).await, None);
                        return Ok(None);
                    }
                    continue;
                }
                Err(e) => {
                    reply.fail_with(&e);
                    halt!(probe(client, link, env, s.tx).await, None);
                    Ok(None)
                }
            };
        }
        // Kept for runs outside the user's block only: one prepared inside it is never
        // reused there, and keeping it would evict the statements runs outside reuse.
        if !reused && !s.tx.block && !s.prepared.off {
            s.prepared.put(sql, stmt.clone());
        }
        // Row-returning: through a portal, inside the user's block or a transaction of its own.
        match portal(client, link, env, s.tx, reply, &stmt, sql, step, opened).await? {
            After::Next(r) => return Ok(Some((r, Outcome::Command(command_tag(sql).0)))),
            After::Done => return Ok(None),
            // Gone (outside the user's block): the server lost it a second time on this
            // connection, so the cache goes off and it runs once more without it.
            After::Stale { all: true, reply: r } if !opened && !fell_back && vanished(s.prepared, env, false) => {
                fell_back = true;
                reply = r;
            }
            After::Stale { all, reply: r } if !prepared_again => {
                // Nothing ran: prepare it again and run it once more.
                if all {
                    s.prepared.clear();
                } else {
                    s.prepared.forget(sql);
                }
                prepared_again = true;
                reply = r;
            }
            // Stale again right after it was prepared again (DDL on the table while it
            // runs): retrying could go on forever, so the user runs it again.
            After::Stale { reply: r, .. } => {
                s.prepared.forget(sql);
                r.fail(DbError::SchemaChanged, false);
                halt!(probe(client, link, env, s.tx).await, None);
                return Ok(None);
            }
            After::Failed => {
                s.prepared.forget(sql);
                halt!(probe(client, link, env, s.tx).await, None);
                return Ok(None);
            }
        }
    }
}

/// Run a statement without rows (one [`returns_no_rows`] knows, or one whose description has
/// no columns): parsed, bound and run as the unnamed statement in one write, inside the user's
/// block after its first statements when they are due, else in a transaction of its own that
/// starts with them when the session wraps its statements (read-only, a search path per
/// transaction).
#[allow(clippy::too_many_arguments)]
async fn no_rows<'a>(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    reply: Reply<'a>,
    sql: &str,
    step: Step,
) -> Result<Option<(Reply<'a>, Outcome)>, Halted<'a>> {
    let token = Some(env.token);
    // Before the last statement, rows a rule returned are all read (0: no limit), so the
    // statement runs to its end; they are not shown.
    let max_rows = if step.last { ask(env.page_size, &None) } else { 0 };
    // Read-only or a search path per transaction: in the user's block, after the first
    // statements if it is new; outside it, in a transaction of its own that starts with
    // them (`BEGIN READ ONLY`, `SET LOCAL search_path …`).
    let (before, after) = if tx.block {
        (tx.take_first(env), None)
    } else if env.wrapped(sql) {
        (env.begin(), Some("COMMIT"))
    } else {
        (Vec::new(), None)
    };
    let executed = client.execute_pipelined_wrapped(sql, max_rows, &before, after);
    let executed = halt!(link.guard(token, executed).await, Some(reply));
    if executed.is_err() && after.is_some() {
        // The failure left the `BEGIN READ ONLY` block open and aborted.
        let _ = halt!(link.guard(None, client.batch_execute("ROLLBACK")).await, Some(reply));
    }
    match executed {
        Ok(done) => match done.statement {
            None => {
                let count = done.rows_affected.unwrap_or(0);
                no_rows_done(client, link, env, tx, reply, sql, step, count).await
            }
            // A rule made it return rows after all (in the text format). Rows beyond a
            // page are gone with the implicit transaction: said so, never shown as all.
            Some(stmt) => {
                let reply = if step.last {
                    let rows = done.rows.len().min(env.page_size);
                    let columns = columns_of(stmt.columns());
                    let paged = reply.page(columns, decode(&done.rows[..rows], &[0]), false, step.start.elapsed());
                    if !done.complete || done.rows.len() > env.page_size {
                        paged.fail(DbError::RowsNotRead(rows as u64), false);
                    }
                    None
                } else {
                    Some((reply, Outcome::Command(command_tag(sql).0)))
                };
                match tx_after(sql) {
                    TxAfter::Same => {}
                    _ => halt!(probe(client, link, env, tx).await, reply.map(|(r, _)| r)),
                }
                Ok(reply)
            }
        },
        Err(e) => {
            reply.fail_in(&e, after.is_some() && env.path.is_some());
            halt!(probe(client, link, env, tx).await, None);
            Ok(None)
        }
    }
}

/// Prepare `sql`. With a search path per transaction the server must parse it with that path
/// (it resolves names then): outside the user's block it is parsed in a transaction of its own
/// that sets it (`BEGIN`, `SET LOCAL …`, Parse, `COMMIT`), in the user's new block after the
/// block's first statements; the same write either way.
///
/// With the cache off (`off`) it is the unnamed statement in the user's block too, after the
/// block's first statements when they are due: nothing of it is left on the server connection
/// once the block ends (outside the block see [`prepare_opened`]).
async fn prepare(
    client: &Client,
    env: &Env<'_>,
    tx: &mut Tx,
    sql: &str,
    off: bool,
) -> Result<Statement, tokio_postgres::Error> {
    if off && tx.block {
        return client.prepare_unnamed_wrapped(sql, &tx.take_first(env), None).await;
    }
    match env.path {
        Some(path) if !tx.block => {
            let before = ["BEGIN".to_string(), path.to_string()];
            client.prepare_wrapped(sql, &before, Some("COMMIT")).await
        }
        Some(_) if tx.first_pending => {
            let first = tx.take_first(env);
            client.prepare_wrapped(sql, &first, None).await
        }
        _ => client.prepare(sql).await,
    }
}

/// Prepare `sql` with the cache off, outside the user's block: the transaction it runs in
/// starts first (`BEGIN`, or the session's own start: `BEGIN READ ONLY`, `SET LOCAL
/// search_path …`), then `sql` is parsed as the unnamed statement, in one write. The block
/// stays open for its portal ([`Client::transaction_opened`]), so a pooler in transaction mode
/// keeps the server connection the statement is on; nothing is left there afterwards. A
/// failure leaves the block open and aborted, for the caller to roll back.
async fn prepare_opened(client: &Client, env: &Env<'_>, sql: &str) -> Result<Statement, tokio_postgres::Error> {
    client.prepare_unnamed_wrapped(sql, &env.begin(), None).await
}

/// A prepared statement without rows on a session that sends statements first in each
/// transaction, when a transaction of its own (`BEGIN READ ONLY`, `SET LOCAL search_path …`,
/// outside the user's block) or the block's first statements (in the user's new block) have
/// to go out with it: bound and run to its end in one write, then its own transaction is
/// committed.
#[allow(clippy::too_many_arguments)]
async fn wrapped_no_rows<'a>(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    reply: Reply<'a>,
    stmt: &Statement,
    sql: &str,
    step: Step,
) -> Result<Option<(Reply<'a>, Outcome)>, Halted<'a>> {
    // A transaction of the session's own that sets the path (not the user's block).
    let path_tx = !tx.block && env.path.is_some();
    let txn = if tx.block {
        client.transaction_in_block_with(tx.take_first(env))
    } else {
        client.transaction_pipelined_with(env.begin())
    };
    let ran = halt!(link.guard(Some(env.token), txn.bind_first_page(stmt, &[], 0)).await, Some(reply));
    let count = match ran {
        Ok(first) => {
            drop(first.portal);
            match halt!(finish(txn, true, link).await, Some(reply)) {
                Ok(()) => first.rows_affected.unwrap_or(0),
                Err(e) => {
                    reply.fail_with(&e);
                    halt!(probe(client, link, env, tx).await, None);
                    return Ok(None);
                }
            }
        }
        Err(e) => {
            let _ = halt!(finish(txn, false, link).await, Some(reply));
            reply.fail_in(&e, path_tx);
            halt!(probe(client, link, env, tx).await, None);
            return Ok(None);
        }
    };
    no_rows_done(client, link, env, tx, reply, sql, step, count).await
}

/// The savepoint an `EXPLAIN ANALYZE` inside the user's block runs in.
const SAVEPOINT: &str = "datarig_explain";

/// Whether `sql` is an `EXPLAIN ANALYZE`, whose statement runs to measure its plan: it runs in a
/// transaction (or, inside the user's block, a savepoint) that is rolled back, whatever it
/// wraps, so it never keeps a change. The UI says so when it wraps more
/// than a read.
fn rolls_back(sql: &str) -> bool {
    let first = lex(sql).into_iter().find(|t| !t.is_trivia());
    first.is_some_and(|t| t.text(sql).eq_ignore_ascii_case("EXPLAIN")) && risk::classify(sql).rolls_back()
}

/// One statement of a run.
#[derive(Clone, Copy)]
struct Step {
    last: bool,
    /// Its place in the run (from 0).
    index: usize,
    start: Instant,
    /// Rows of a row-returning last statement to read and drop before its first page (a
    /// `Resume`); 0 for a run.
    skip: u64,
    /// The app runs it again for the user (`Resume`), not the user.
    resume: bool,
    /// A last statement's result with more rows keeps its portal open outside the user's block
    /// (`PagingMode::Hold`); otherwise it ends with its first page.
    hold: bool,
}

/// How a row-returning statement ended.
enum After<'a> {
    /// Not the last statement: go on with the next, with the run's reply.
    Next(Reply<'a>),
    /// The run is over (answered).
    Done,
    /// It failed (answered); the transaction state must be asked for.
    Failed,
    /// Its prepared statement no longer fits its result or is gone ([`stale`]): nothing ran
    /// and nothing was answered. `all`: the server has none of the session's statements
    /// ([`gone`]).
    Stale { all: bool, reply: Reply<'a> },
}

/// Run a row-returning statement through a portal. The last statement of a run held
/// (`PagingMode::Hold`, or inside the user's block) keeps the portal open while more rows follow
/// and serves `FetchMore` until the result is complete, a `ClosePortal` for it arrives, or
/// another command ends it (that one is processed next); one not held ends its portal and
/// transaction with its first page (`DbEvent::Released` before the page when more rows follow).
#[allow(clippy::too_many_arguments)]
async fn portal<'a>(
    client: &mut Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    reply: Reply<'a>,
    stmt: &Statement,
    sql: &str,
    step: Step,
    opened: bool,
) -> Result<After<'a>, Halted<'a>> {
    let Step { last, start, index, skip, resume, hold } = step;
    // `EXPLAIN ANALYZE` ran its statement only to measure it: its transaction is rolled back.
    let keep = !rolls_back(sql);
    let id = reply.id;
    let token = Some(env.token);
    #[cfg(test)]
    if let Some(hook) = env.before_bind {
        hook().await;
    }
    // Outside the user's block the portal needs a transaction of its own; its `BEGIN` (`BEGIN
    // READ ONLY` on a read-only session, then a search path set per transaction) goes out with
    // the first page. It is reported only if it stays open (a result with more rows): one that
    // ends within the run would only make the indicator flicker. In the user's new block the
    // session's first statements (`SET TRANSACTION READ ONLY`, the path) go out first.
    //
    // A statement the app runs again for the user inside the user's block (`Resume`) runs under
    // the savepoint `run` set for it ([`RESUME_SAVEPOINT`]), as a count does: released when its
    // portal ends, rolled back to when it fails or is cancelled (on its first page, while it
    // skips, or on a later page), so it never aborts the user's transaction nor undoes what the
    // user did in it.
    let savepoint = resume && tx.block;
    //
    // No hold (`PagingMode::NoHold`), outside the user's block: the last statement's portal and
    // its transaction end with its first page, and nothing stays open on the server while the
    // user reads it (no lock, no snapshot, no transaction). The `COMMIT` (`ROLLBACK` for `EXPLAIN
    // ANALYZE`) goes out after the Execute in the same write, so the page is read and the
    // transaction ended in one round trip; a `Resume` whose skipped rows need more than its first
    // request reads them first and ends it right after.
    let release = last && !hold && !tx.block;
    //
    // With the cache off its transaction began before the statement was parsed (`opened`,
    // [`prepare_opened`]).
    let mut txn = if tx.block {
        client.transaction_in_block_with(tx.take_first(env))
    } else if opened {
        client.transaction_opened()
    } else if env.wraps() {
        client.transaction_pipelined_with(env.begin())
    } else {
        client.transaction_pipelined()
    };
    let page_size = env.page_size;
    let mut carry = None;
    // A `Resume` reads the rows it skips first, a chunk at a time, and the page after them
    // with the last chunk (one round trip when they fit in one).
    let max_rows = if skip > 0 { skip_chunk(skip, page_size) } else { ask(page_size, &carry) };
    let formats = formats_of(stmt);
    // The first request reads everything the page needs (the skipped rows, the page and the
    // row that tells whether more follow): it can end the transaction too.
    let end_now = release && skip.saturating_add(page_size as u64 + 1) <= max_rows as u64;
    let bound = if end_now {
        let end = if keep { "COMMIT" } else { "ROLLBACK" };
        halt!(link.guard(token, txn.bind_first_page_then(stmt, &formats, max_rows, end)).await, Some(reply))
    } else {
        halt!(link.guard(token, txn.bind_first_page(stmt, &formats, max_rows)).await, Some(reply))
    };
    let first = match bound {
        Ok(first) => first,
        Err(e) => {
            if savepoint {
                // An aborted block refuses this too, and stays as it was.
                let _ = halt!(link.guard(None, txn.batch_execute(RESUME_UNDO)).await, Some(reply));
            }
            let _ = halt!(finish(txn, false, link).await, Some(reply));
            if stale(&e) && !tx.block {
                env.report(tx, tx.block);
                return Ok(After::Stale { all: gone(&e), reply });
            }
            reply.fail_in(&e, !tx.block && env.path.is_some());
            return Ok(After::Failed);
        }
    };
    let FirstPage { portal, columns, rows, complete, .. } = first;
    let rows = if skip > 0 && last {
        // Drop `skip` rows, then ask for what the page after them still needs. A result that
        // now ends within them answers with an empty last page.
        let mut rows = rows;
        let mut left = skip;
        let mut ended = complete;
        let mut failed = None;
        loop {
            let n = rows.len() as u64;
            if n >= left {
                rows.drain(..usize::try_from(left).unwrap_or(usize::MAX));
                left = 0;
                break;
            }
            left -= n;
            rows.clear();
            if ended {
                break;
            }
            let asked = skip_chunk(left, page_size);
            match halt!(link.guard(token, txn.query_portal(&portal, asked)).await, Some(reply)) {
                Ok(more) => {
                    ended = (more.len() as i32) < asked;
                    rows = more;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        let want = ask(page_size, &carry) as usize;
        if failed.is_none() && left == 0 && !ended && rows.len() < want {
            let asked = i32::try_from(want - rows.len()).unwrap_or(i32::MAX);
            match halt!(link.guard(token, txn.query_portal(&portal, asked)).await, Some(reply)) {
                Ok(more) => rows.extend(more),
                Err(e) => failed = Some(e),
            }
        }
        if let Some(e) = failed {
            drop(portal);
            if savepoint {
                let _ = halt!(link.guard(None, txn.batch_execute(RESUME_UNDO)).await, Some(reply));
            }
            let _ = halt!(finish(txn, false, link).await, Some(reply));
            reply.fail_with(&e);
            return Ok(After::Failed);
        }
        if left > 0 {
            drop(portal);
            let paged = reply.page(columns_of(&columns), Vec::new(), false, start.elapsed());
            return committed(txn, keep, savepoint, link, env, tx, paged).await;
        }
        rows
    } else {
        rows
    };
    if !last {
        // A statement before the last runs to its end, as psql runs it (volatile functions and
        // row locks take effect for every row): all its rows are fetched, page by page, and go
        // to the UI as the statement's own result.
        let (page, mut more) = split_page(&mut carry, rows, page_size);
        let (columns, rows) = (Some(columns_of(&columns)), decode(&page, &formats));
        let _ = env.events.send(DbEvent::StepRows { id, index, columns, rows, more });
        while more {
            // Cancelled between two pages: the statement stops, the rest does not run.
            if env.cancel.load(Ordering::SeqCst) {
                drop(portal);
                let _ = halt!(finish(txn, false, link).await, Some(reply));
                reply.fail(DbError::Cancelled, true);
                return Ok(After::Failed);
            }
            match halt!(link.guard(token, txn.query_portal(&portal, ask(page_size, &carry))).await, Some(reply)) {
                Ok(fetched) => {
                    let (page, m) = split_page(&mut carry, fetched, page_size);
                    more = m;
                    let rows = decode(&page, &formats);
                    let _ = env.events.send(DbEvent::StepRows { id, index, columns: None, rows, more });
                }
                Err(e) => {
                    drop(portal);
                    let _ = halt!(finish(txn, false, link).await, Some(reply));
                    reply.fail_with(&e);
                    return Ok(After::Failed);
                }
            }
        }
        drop(portal);
        // Committed, the run goes on with the next statement.
        return match halt!(finish(txn, keep, link).await, Some(reply)) {
            Ok(()) => {
                env.report(tx, tx.block);
                Ok(After::Next(reply))
            }
            Err(e) => {
                reply.fail_with(&e);
                Ok(After::Failed)
            }
        };
    }
    let (rows, mut more) = split_page(&mut carry, rows, page_size);
    let (columns, rows) = (columns_of(&columns), decode(&rows, &formats));
    if !more {
        // Complete in one page: nothing stays open. A plain read is shown at once and its
        // transaction ends right after; rows of a statement that may write are shown once its
        // `COMMIT` succeeded.
        drop(portal);
        if plain_read(sql) || tx.block {
            let paged = reply.page(columns, rows, false, start.elapsed());
            return committed(txn, keep, savepoint, link, env, tx, paged).await;
        }
        return match halt!(finish(txn, keep, link).await, Some(reply)) {
            Ok(()) => {
                reply.page(columns, rows, false, start.elapsed());
                env.report(tx, tx.block);
                Ok(After::Done)
            }
            Err(e) => {
                reply.fail_with(&e);
                Ok(After::Failed)
            }
        };
    }
    if release {
        // Not held: the portal goes and its transaction ends now (already, when the first
        // request ended it), before the page is the answer; past the page the app can only run
        // the statement again.
        drop(portal);
        if let Err(e) = halt!(finish(txn, keep, link).await, Some(reply)) {
            reply.fail_with(&e);
            return Ok(After::Failed);
        }
        env.report(tx, tx.block);
        let _ = env.events.send(DbEvent::Released { id });
        reply.page(columns, rows, true, start.elapsed());
        return Ok(After::Done);
    }
    // The portal stays open for more rows, and with it the transaction that holds it.
    env.report(tx, true);
    let paged = reply.page(columns, rows, true, start.elapsed());
    while more {
        match link.next().await {
            Next::Command(DbCommand::FetchMore { id: fid }) if fid == id => {
                let t0 = Instant::now();
                match halt!(link.guard(token, txn.query_portal(&portal, ask(page_size, &carry))).await, None) {
                    Ok(rows) => {
                        let (rows, m) = split_page(&mut carry, rows, page_size);
                        more = m;
                        let _ = env.events.send(DbEvent::Page {
                            id,
                            columns: None,
                            rows: decode(&rows, &formats),
                            more,
                            elapsed: t0.elapsed(),
                        });
                    }
                    Err(e) => {
                        drop(portal);
                        if savepoint {
                            let _ = halt!(link.guard(None, txn.batch_execute(RESUME_UNDO)).await, None);
                        }
                        let _ = halt!(finish(txn, false, link).await, None);
                        paged.fail_with(&e);
                        return Ok(After::Failed);
                    }
                }
            }
            // The UI closes a result nobody paged for a while (policy `paging_idle_timeout`):
            // it ends like a finished one, except that the user's own block stays open.
            Next::Command(DbCommand::ClosePortal { id: cid }) if cid == id => break,
            // A count of this result's rows runs in the portal's transaction, under a
            // savepoint: the portal goes on paging after it, whatever it answered.
            Next::Command(DbCommand::Count { id: cid, sql }) => {
                let (result, snapshot) = halt!(counted_in(&txn, link, env, &sql).await, None);
                let _ = env.events.send(DbEvent::Counted { id: cid, result, snapshot });
            }
            // So is the allowlist's question, which leaves the portal paging too.
            Next::Command(DbCommand::CheckRepeat { id: cid, sql }) => {
                let result = halt!(checked_repeat_in(&txn, link, env, &sql).await, None);
                let _ = env.events.send(DbEvent::RepeatChecked { id: cid, result });
            }
            Next::Command(DbCommand::FetchMore { .. } | DbCommand::ClosePortal { .. }) => {}
            Next::Command(other) => {
                link.requeue(other);
                break;
            }
            // Dropping the connection rolls the implicit transaction back.
            Next::Closed => return Err(Halted { stop: Stop::Closed, reply: None }),
            Next::Lost(reason) => return Err(Halted { stop: Stop::Lost(reason), reply: None }),
        }
    }
    drop(portal);
    committed(txn, keep, savepoint, link, env, tx, paged).await
}

/// Commit the portal's transaction after its first page was the answer (roll it back unless
/// `keep`), and report the state; a failed `COMMIT` fails the statement (after its rows, which
/// the UI then replaces with the error). `savepoint`: a resumed statement's savepoint in the
/// user's block is released first.
async fn committed<'a>(
    txn: Transaction<'_>,
    keep: bool,
    savepoint: bool,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    paged: Paged<'_>,
) -> Result<After<'a>, Halted<'a>> {
    if savepoint && let Err(e) = halt!(link.guard(None, txn.batch_execute(RESUME_RELEASE)).await, None) {
        paged.fail_with(&e);
        return Ok(After::Failed);
    }
    match halt!(finish(txn, keep, link).await, None) {
        Ok(()) => {
            env.report(tx, tx.block);
            Ok(After::Done)
        }
        Err(e) => {
            paged.fail_with(&e);
            Ok(After::Failed)
        }
    }
}

/// The savepoint a statement run again for the user (`Resume`) runs under in the user's block
/// (set in the request of the allowlist's check, [`check_text`]).
const RESUME_SAVEPOINT: &str = "SAVEPOINT datarig_resume";

/// Leaves [`RESUME_SAVEPOINT`] when the statement ran to its end or its portal closed.
const RESUME_RELEASE: &str = "RELEASE SAVEPOINT datarig_resume";

/// Back to before [`RESUME_SAVEPOINT`] when the statement failed or was cancelled: the user's
/// block is as it was (not aborted, its changes kept).
const RESUME_UNDO: &str = "ROLLBACK TO SAVEPOINT datarig_resume; RELEASE SAVEPOINT datarig_resume";

/// Rows a `Resume` reads (and drops) per request while it skips.
const SKIP_CHUNK: u64 = 10_000;

/// Rows to ask for while skipping, `left` still to skip: the page after them too (and the row
/// that tells whether more follow), at most [`SKIP_CHUNK`] at a time. So the last chunk never
/// reads past that page, and the portal stays where the page ends.
fn skip_chunk(left: u64, page_size: usize) -> i32 {
    let want = left.saturating_add(page_size as u64 + 1);
    i32::try_from(want.min(SKIP_CHUNK.max(page_size as u64 + 1))).unwrap_or(i32::MAX)
}

/// The savepoint a count or the allowlist's question runs under inside a transaction: a name
/// the user does not use, `datarig_count_` and 16 random hex digits, made once per process.
pub fn count_savepoint() -> &'static str {
    use std::hash::{BuildHasher, Hasher};
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let n = std::collections::hash_map::RandomState::new().build_hasher().finish();
        format!("datarig_count_{n:016x}")
    })
}

/// The count in a simple query's answer (its first row's first value).
fn count_of(msgs: &[SimpleQueryMessage]) -> Result<u64, DbError> {
    msgs.iter()
        .find_map(|m| match m {
            SimpleQueryMessage::Row(r) => r.get(0).and_then(|v| v.parse().ok()),
            _ => None,
        })
        .ok_or(DbError::NoResult)
}

/// A failed count as the UI hears it (a cancel is `Cancelled`).
fn count_error(e: &tokio_postgres::Error) -> DbError {
    if is_cancel(e) { DbError::Cancelled } else { db_error(e) }
}

/// Whether the session runs `sql` as a count: only the app's `SELECT count(*)` of a statement
/// on the allowlist (`risk::repeat`), checked again here, because it goes out as one simple
/// query with the savepoint around it.
fn countable(sql: &str) -> bool {
    risk::repeat::repeatable(sql).is_ok()
}

/// Count with `sql` while no portal is open. Inside the user's block it runs
/// under a savepoint, rolled back to on failure, so a failed count never aborts the block (made
/// read-only first on a read-only session, as the block's first statement); outside it, as a
/// statement of its own (`BEGIN READ ONLY … COMMIT` on a read-only session). The allowlist's
/// question to the server goes first ([`check_text`]), in the savepoint's request inside the
/// block. One round trip when it succeeds. Also answers whether it counted in a snapshot fixed
/// for the transaction (the user's block at `REPEATABLE READ` or `SERIALIZABLE`, asked with
/// [`ISOLATION`] in the same request); outside a block it never does (it counts the rows
/// committed when it runs).
async fn count(
    client: &Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    sql: &str,
) -> Result<(Result<u64, DbError>, bool), Closed> {
    if !countable(sql) {
        return Ok((Err(DbError::NotSupported), false));
    }
    let check = match check_text(sql) {
        Ok(check) => check,
        Err(why) => return Ok((Err(why), false)),
    };
    let sp = env.savepoint;
    if tx.block {
        let first = tx.take_first_text(env);
        let asked = format!("{first}SAVEPOINT {sp}; {ISOLATION}; {check}");
        let text = format!("{sql}; RELEASE SAVEPOINT {sp}");
        let (fixed, answer) = checked(link, env, client.simple_query(&asked)).await?;
        let counted = match answer {
            Ok(()) => match link.guard(Some(env.token), client.simple_query(&text)).await? {
                Ok(msgs) => return Ok((count_of(&msgs), fixed)),
                Err(e) => count_error(&e),
            },
            Err(why) => why,
        };
        // Back to before the count (an aborted block refuses this too, and stays as it was);
        // the probe tells the block's state either way.
        let _ = link.guard(None, client.batch_execute(&count_undo(env))).await?;
        probe(client, link, env, tx).await?;
        return Ok((Err(counted), false));
    }
    // A search path per transaction goes first in each simple query (one implicit transaction).
    let path = env.path_text();
    if let (_, Err(why)) = checked(link, env, client.simple_query(&format!("{path}{check}"))).await? {
        return Ok((Err(why), false));
    }
    let text = if env.read_only { format!("{BEGIN_READ_ONLY}; {path}{sql}; COMMIT") } else { format!("{path}{sql}") };
    let counted = match link.guard(Some(env.token), client.simple_query(&text)).await? {
        Ok(msgs) => count_of(&msgs),
        Err(e) => {
            if env.read_only {
                // The failure left the `BEGIN READ ONLY` block open and aborted.
                let _ = link.guard(None, client.batch_execute("ROLLBACK")).await?;
            }
            Err(count_error(&e))
        }
    };
    Ok((counted, false))
}

/// Count with `sql` while a portal is open, in the portal's transaction (the user's block, or
/// the one of its own the portal needs), under a savepoint rolled back to on failure: the
/// portal goes on paging after it and the transaction is not aborted, whatever the count
/// answers. Also answers whether that transaction has one snapshot for its whole life
/// (`REPEATABLE READ` or `SERIALIZABLE`, asked with [`ISOLATION`] in the same request).
async fn counted_in(
    txn: &Transaction<'_>,
    link: &mut Link,
    env: &Env<'_>,
    sql: &str,
) -> Result<(Result<u64, DbError>, bool), Closed> {
    if !countable(sql) {
        return Ok((Err(DbError::NotSupported), false));
    }
    let check = match check_text(sql) {
        Ok(check) => check,
        Err(why) => return Ok((Err(why), false)),
    };
    let sp = env.savepoint;
    let asked = format!("SAVEPOINT {sp}; {ISOLATION}; {check}");
    let text = format!("{sql}; RELEASE SAVEPOINT {sp}");
    let (fixed, answer) = checked(link, env, txn.simple_query(&asked)).await?;
    let counted = match answer {
        Ok(()) => match link.guard(Some(env.token), txn.simple_query(&text)).await? {
            Ok(msgs) => return Ok((count_of(&msgs), fixed)),
            Err(e) => count_error(&e),
        },
        Err(why) => why,
    };
    let _ = link.guard(None, txn.batch_execute(&count_undo(env))).await?;
    Ok((Err(counted), false))
}

/// The allowlist's question about `sql` alone (`DbCommand::CheckRepeat`) while no portal is
/// open: inside the user's block under a savepoint rolled back to after it (so a failure does
/// not abort the block), otherwise as a query of its own. One round trip.
async fn check_repeat(
    client: &Client,
    link: &mut Link,
    env: &Env<'_>,
    tx: &mut Tx,
    sql: &str,
) -> Result<Result<(), DbError>, Closed> {
    let check = match check_text(sql) {
        Ok(check) => check,
        Err(why) => return Ok(Err(why)),
    };
    let sp = env.savepoint;
    // An aborted block takes nothing but its end: not asked, refused (the user is asked).
    if tx.block && tx.aborted {
        return Ok(Err(DbError::NotRepeatable(risk::repeat::NotRepeatable::Unreadable)));
    }
    if tx.block {
        let first = tx.take_first_text(env);
        let asked = format!("{first}SAVEPOINT {sp}; {check}");
        let (_, answer) = checked(link, env, client.simple_query(&asked)).await?;
        let undone = link.guard(None, client.batch_execute(&count_undo(env))).await?;
        if answer.is_err() || undone.is_err() {
            probe(client, link, env, tx).await?;
        }
        return Ok(answer);
    }
    let path = env.path_text();
    Ok(checked(link, env, client.simple_query(&format!("{path}{check}"))).await?.1)
}

/// [`check_repeat`] while a portal is open, in its transaction under a savepoint: the portal
/// goes on paging after it.
async fn checked_repeat_in(
    txn: &Transaction<'_>,
    link: &mut Link,
    env: &Env<'_>,
    sql: &str,
) -> Result<Result<(), DbError>, Closed> {
    let check = match check_text(sql) {
        Ok(check) => check,
        Err(why) => return Ok(Err(why)),
    };
    let sp = env.savepoint;
    let asked = format!("SAVEPOINT {sp}; {check}");
    let (_, answer) = checked(link, env, txn.simple_query(&asked)).await?;
    let _ = link.guard(None, txn.batch_execute(&count_undo(env))).await?;
    Ok(answer)
}

/// Asked with the allowlist's question before a count in a transaction: its isolation, so the
/// count says whether it saw the transaction's one snapshot. No round trip of its own.
const ISOLATION: &str = "SELECT pg_catalog.current_setting('transaction_isolation')";

/// Back to before the driver's count savepoint ([`Env::savepoint`]) when a count failed, was
/// cancelled or was refused, or after the allowlist's question.
fn count_undo(env: &Env<'_>) -> String {
    let sp = env.savepoint;
    format!("ROLLBACK TO SAVEPOINT {sp}; RELEASE SAVEPOINT {sp}")
}

/// The query that asks the server what only it can tell for the allowlist about `sql`
/// (`risk::repeat::check_query` of the objects it names) right before the app runs it again or
/// counts it. The statement's own text is refused here when it is off the allowlist or a name
/// cannot be sent.
fn check_text(sql: &str) -> Result<String, DbError> {
    use risk::repeat::{NotRepeatable, check_query, names};
    let names = names(sql).map_err(DbError::NotRepeatable)?;
    check_query(&names).ok_or(DbError::NotRepeatable(NotRepeatable::Unreadable))
}

/// What the server answered to [`check_text`]: a row (of its two columns) says why the
/// statement may not run. A one-column row is the answer to [`ISOLATION`] asked with it.
fn check_answer(msgs: &[SimpleQueryMessage]) -> Result<(), DbError> {
    use risk::repeat::NotRepeatable;
    match msgs.iter().find_map(|m| match m {
        SimpleQueryMessage::Row(r) if r.len() == 2 => {
            Some((r.get(0).unwrap_or_default(), r.get(1).unwrap_or_default()))
        }
        _ => None,
    }) {
        None => Ok(()),
        Some((kind, name)) => {
            Err(DbError::NotRepeatable(NotRepeatable::from_check(kind, name).unwrap_or(NotRepeatable::Unreadable)))
        }
    }
}

/// Send the request of [`check_text`] (cancellable) and read its answer: `Err` when the
/// statement may not run, or asking failed (a cancel is `Cancelled`); with whether the
/// transaction has one snapshot, when [`ISOLATION`] was asked with it.
async fn checked(
    link: &mut Link,
    env: &Env<'_>,
    asked: impl std::future::Future<Output = Result<Vec<SimpleQueryMessage>, tokio_postgres::Error>>,
) -> Result<(bool, Result<(), DbError>), Closed> {
    Ok(match link.guard(Some(env.token), asked).await? {
        Ok(msgs) => {
            let fixed = msgs.iter().any(|m| {
                matches!(m, SimpleQueryMessage::Row(r) if r.len() == 1
                    && matches!(r.get(0), Some("repeatable read" | "serializable")))
            });
            (fixed, check_answer(&msgs))
        }
        Err(e) => (false, Err(count_error(&e))),
    })
}

/// Rows to ask the portal for: one more than a page (less the row carried over), so a result
/// that fits in one page is known to be complete and its portal ends at once.
fn ask(page_size: usize, carry: &Option<Row>) -> i32 {
    i32::try_from(page_size + 1 - usize::from(carry.is_some())).unwrap_or(i32::MAX)
}

/// The next page from the carried-over row and the rows just fetched, and whether more follow.
/// The row beyond a full page is carried over to start the next one.
fn split_page<T>(carry: &mut Option<T>, fetched: Vec<T>, page_size: usize) -> (Vec<T>, bool) {
    let mut rows: Vec<T> = carry.take().into_iter().chain(fetched).collect();
    if rows.len() > page_size {
        *carry = rows.pop();
        rows.truncate(page_size);
        (rows, true)
    } else {
        (rows, false)
    }
}

#[cfg(test)]
mod tests;
