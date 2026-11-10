//! One session: the thread that owns its connection, and the loop that answers its commands in
//! order. The connection never leaves the thread; the UI's side holds only the channel and the
//! canceller.

use crate::{meta, open, query};
use datarig_core::driver::{Canceller, ConnectOptions, DbCommand, DbError, DbEvent, SessionRole, SessionStatement};
use rusqlite::{Connection, InterruptHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Virtual machine steps between two looks at the cancel flag (SQLite's progress handler).
const STEPS_BETWEEN_CHECKS: i32 = 10_000;

/// Stops the statement a session runs. The UI's side holds the only strong reference; the
/// session's thread holds a weak one, so a session the UI dropped is noticed and its statement
/// stops.
#[derive(Default)]
pub(crate) struct SqliteCanceller {
    /// The UI asked to stop the run in progress (cleared when a run starts).
    pub(crate) asked: AtomicBool,
    interrupt: Mutex<Option<InterruptHandle>>,
}

impl Canceller for SqliteCanceller {
    fn cancel(&self) {
        self.asked.store(true, Ordering::SeqCst);
        if let Ok(handle) = self.interrupt.lock()
            && let Some(handle) = handle.as_ref()
        {
            handle.interrupt();
        }
    }
}

impl SqliteCanceller {
    fn attach(&self, conn: &Connection) {
        if let Ok(mut handle) = self.interrupt.lock() {
            *handle = Some(conn.get_interrupt_handle());
        }
    }
}

/// What the session's thread answers with.
pub(crate) struct Ctx {
    events: UnboundedSender<DbEvent>,
    canceller: Weak<SqliteCanceller>,
}

impl Ctx {
    pub(crate) fn send(&self, event: DbEvent) {
        // A closed channel means the session was dropped; nothing is waiting for the answer.
        let _ = self.events.send(event);
    }

    /// Whether the UI asked to stop the run in progress.
    pub(crate) fn asked(&self) -> bool {
        self.canceller.upgrade().is_some_and(|c| c.asked.load(Ordering::SeqCst))
    }

    /// A run starts: an earlier request to stop does not apply to it.
    pub(crate) fn start_run(&self) {
        if let Some(c) = self.canceller.upgrade() {
            c.asked.store(false, Ordering::SeqCst);
        }
    }
}

/// The session's thread: opens the file, starts the connection, then serves commands until the
/// UI drops the session.
pub(crate) fn run(
    file: PathBuf,
    role: SessionRole,
    opts: ConnectOptions,
    events: UnboundedSender<DbEvent>,
    mut rx: UnboundedReceiver<DbCommand>,
    canceller: Weak<SqliteCanceller>,
) {
    // Metadata sessions only read; a query session follows the profile's policy.
    let read_only = role != SessionRole::Query || opts.read_only;
    let wait = opts.lock_wait.unwrap_or(open::BUSY_WAIT);
    let mut conn = match open::open(&file, read_only, wait) {
        Ok(conn) => conn,
        Err(error) => return connect_failed(&events, error),
    };
    if role == SessionRole::Query
        && let Err(error) = start_statements(&conn, &opts.session_sql)
    {
        return connect_failed(&events, error);
    }
    let Some(own) = canceller.upgrade() else {
        // The UI dropped the session before it opened: nothing to serve.
        return;
    };
    own.attach(&conn);
    drop(own);
    let stop_flag = canceller.clone();
    let stop = move || stop_flag.upgrade().is_none_or(|c| c.asked.load(Ordering::SeqCst));
    if let Err(error) = conn.progress_handler(STEPS_BETWEEN_CHECKS, Some(stop)) {
        return connect_failed(&events, open::server(error));
    }
    let _ = events.send(DbEvent::Connected);
    let ctx = Ctx { events, canceller };
    // Like the other drivers' metadata sessions: the schema list and the completion catalog are
    // sent as soon as the session is up, so the explorer's tree loads without a request.
    if role == SessionRole::Meta {
        meta::at_connect(&conn, &ctx);
    }
    while let Some(cmd) = rx.blocking_recv() {
        serve(&conn, &ctx, role, cmd);
    }
}

fn connect_failed(events: &UnboundedSender<DbEvent>, error: DbError) {
    let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
}

/// The profile's session statements (`session_sql`), each once and in order; a failure fails the
/// connect.
fn start_statements(conn: &Connection, statements: &[SessionStatement]) -> Result<(), DbError> {
    for (index, statement) in statements.iter().enumerate() {
        conn.execute_batch(&statement.sql).map_err(|e| DbError::SessionSql {
            index,
            statement: statement.sql.clone(),
            error: Box::new(open::server(e)),
        })?;
    }
    Ok(())
}

/// Answers one command. A command this session does not serve gets the protocol's
/// "not supported" answer.
fn serve(conn: &Connection, ctx: &Ctx, role: SessionRole, cmd: DbCommand) {
    match (role, cmd) {
        (SessionRole::Meta, cmd @ (DbCommand::LoadSchemas | DbCommand::LoadObjects { .. })) => {
            meta::answer(conn, ctx, cmd)
        }
        (SessionRole::Meta, cmd @ (DbCommand::LoadCatalog | DbCommand::LoadDdl { .. })) => meta::answer(conn, ctx, cmd),
        (SessionRole::Query, DbCommand::Execute { id, statements, tx, .. }) => {
            query::execute(conn, ctx, id, statements, tx)
        }
        // Nothing is reported for a portal: a result is one page, there is no cursor to close.
        (SessionRole::Query, DbCommand::ClosePortal { .. }) => {}
        (SessionRole::Query, DbCommand::Export { id, sql, sink, batch, .. }) => {
            query::export(conn, ctx, id, &sql, &sink, batch)
        }
        (_, cmd) => ctx.send(cmd.not_supported()),
    }
}
