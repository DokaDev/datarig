//! Opening sessions and test connections: profile -> the server, its credentials and the
//! session's settings, through the session's route (directly or a tunnel's dialer), and the
//! canceller (`KILL QUERY` on a connection of its own).

use crate::link::Link;
use crate::route::Route;
use crate::session::{self, CONNECT_GUARD, Killer, Opened, Settings, Target, connect_error, my_error};
use datarig_core::driver::{
    Canceller, Capabilities, ConnectOptions, DbCommand, DbError, DbEvent, Driver, Hierarchy, PingError, PingInfo,
    Session, SessionRole,
};
use datarig_core::fault::Fault;
use datarig_core::panics::caught;
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::transport::DialerRef;
use futures::FutureExt;
use futures::future::BoxFuture;
use mysql_async::prelude::Queryable;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

pub struct MyDriver;

/// Cancels the query session's statement: flags the request for the session (a run of several
/// statements stops before its next one) and, while a statement runs, has the server stop it
/// (`KILL QUERY` of the session's connection, from a connection of its own).
#[derive(Default)]
pub(crate) struct MyCanceller {
    killer: Mutex<Option<Killer>>,
    /// The UI asked to cancel (cleared by the session when a run starts).
    pub(crate) asked: Arc<AtomicBool>,
    /// A statement is on its way to the server or running there: only then is there anything
    /// to stop (a `KILL QUERY` that arrives between statements stops nothing).
    pub(crate) busy: Arc<AtomicBool>,
}

impl Canceller for MyCanceller {
    fn cancel(&self) {
        self.asked.store(true, Ordering::SeqCst);
        if !self.busy.load(Ordering::SeqCst) {
            return;
        }
        let killer = self.killer.lock().ok().and_then(|g| g.clone());
        if let Some(killer) = killer {
            // Still this run's statement: a run that begins since clears `asked`.
            let (asked, busy) = (self.asked.clone(), self.busy.clone());
            let still = move || asked.load(Ordering::SeqCst) && busy.load(Ordering::SeqCst);
            // Unreported either way: a panic in it is only kept off the screen.
            tokio::spawn(AssertUnwindSafe(caught(async move { killer.kill_query_if(still).await })).catch_unwind());
        }
    }
}

impl Driver for MyDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            server_paging: true,
            cancel: true,
            introspection: true,
            key_metadata: false,
            contexts: false,
            structure: true,
            ddl: true,
            language: Language::Sql(Dialect::MySql(MySqlMode::default())),
            hierarchy: Hierarchy::SchemaOnly,
            explain: None,
            reads_held_to_end: true,
        }
    }

    fn connect(
        &self,
        cfg: &ConnectionConfig,
        role: SessionRole,
        opts: ConnectOptions,
        events: UnboundedSender<DbEvent>,
    ) -> Session {
        let (tx, rx) = unbounded_channel();
        let canceller = Arc::new(MyCanceller::default());
        // Read on the session's task: the profile's key file is a file.
        let cfg = cfg.clone();
        let c2 = canceller.clone();
        tokio::spawn(async move {
            let connected = AtomicBool::new(false);
            let session = async {
                match Target::of(&cfg) {
                    Err(error) => {
                        let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
                    }
                    Ok(target) => run_session(target, role, opts, events.clone(), rx, c2, &connected).await,
                }
            };
            guarded(&events, &connected, session).await;
        });
        Session::new(self.capabilities(), role, tx, canceller)
    }

    fn ping(
        &self,
        cfg: &ConnectionConfig,
        timeout: Duration,
        dialer: Option<DialerRef>,
    ) -> BoxFuture<'static, Result<PingInfo, PingError>> {
        let cfg = cfg.clone();
        Box::pin(async move {
            let target = Target::of(&cfg).map_err(PingError::Failed)?;
            let route = Route::new(&target.host, target.port, dialer);
            let opts = target.opts(target.database.as_deref(), "datarig-test", &route);
            let start = Instant::now();
            let attempt = async {
                let stream = route.dial(timeout).await?;
                let (_wire, stream) = crate::wire::Wire::new(stream);
                let mut conn = mysql_async::Conn::connect_with_stream(opts, Box::new(stream))
                    .await
                    .map_err(|e| connect_error(&e).0)?;
                let server = session::Server { version: conn.server_version(), mariadb: conn.is_mariadb() };
                let version: Result<Option<String>, _> = match server.check() {
                    Ok(()) => conn.query_first("SELECT VERSION()").await.map_err(|e| my_error(&e)),
                    Err(e) => Err(e),
                };
                session::quit(conn).await;
                Ok::<_, DbError>(version?.unwrap_or_else(|| server.label()))
            };
            // A panic in the attempt fails the test like any other error.
            match AssertUnwindSafe(caught(tokio::time::timeout(timeout, attempt))).catch_unwind().await {
                Ok(Ok(Ok(server_version))) => Ok(PingInfo { server_version, latency: start.elapsed() }),
                Ok(Ok(Err(e))) => Err(PingError::Failed(e)),
                Ok(Err(_)) => Err(PingError::Timeout(timeout)),
                Err(_) => Err(PingError::Failed(internal_error())),
            }
        })
    }
}

/// Run a session's task, `session` (`connected` is set before it sends `Connected`). A panic
/// in it (a bug, here or in a crate under the driver) still ends the session with one reported
/// failure, [`internal_error`]: `ConnectFailed` before it connected, `Lost` after. It runs
/// [`caught`], so the binary's panic hook leaves the terminal alone and prints nothing.
pub(crate) async fn guarded(
    events: &UnboundedSender<DbEvent>,
    connected: &AtomicBool,
    session: impl Future<Output = ()>,
) {
    if AssertUnwindSafe(caught(session)).catch_unwind().await.is_ok() {
        return;
    }
    let error = internal_error();
    let _ = events.send(match connected.load(Ordering::SeqCst) {
        true => DbEvent::Lost { error },
        false => DbEvent::ConnectFailed { error, auth: false },
    });
}

/// What a panic in the driver reads as. Its message is left out: it could quote anything the
/// task held, a password among it.
fn internal_error() -> DbError {
    DbError::Connection(Fault::other("internal error in the MySQL driver (it panicked)"))
}

/// The database a session starts in: the one its context names, else the profile's.
fn start_database(target: &Target, opts: &ConnectOptions) -> Option<String> {
    opts.context.schema.clone().or_else(|| opts.context.database.clone()).or_else(|| target.database.clone())
}

async fn run_session(
    target: Target,
    role: SessionRole,
    opts: ConnectOptions,
    events: UnboundedSender<DbEvent>,
    rx: UnboundedReceiver<DbCommand>,
    canceller: Arc<MyCanceller>,
    connected: &AtomicBool,
) {
    let route = Route::new(&target.host, target.port, opts.dialer.clone());
    let database = start_database(&target, &opts);
    let my_opts = target.opts(database.as_deref(), &opts.application_name, &route);
    let select_limit = (role == SessionRole::Query).then(|| opts.page_size as u64 + 1);
    let settings = Settings { role, read_only: opts.read_only, select_limit };
    // A server that accepts TCP but never answers would otherwise keep this task alive forever
    // (the UI gives up earlier).
    let opened = match tokio::time::timeout(CONNECT_GUARD, session::open(&route, my_opts.clone(), settings)).await {
        Ok(Ok(opened)) => opened,
        Ok(Err((error, auth))) => {
            let _ = events.send(DbEvent::ConnectFailed { error, auth });
            return;
        }
        Err(_) => {
            let _ = events.send(DbEvent::ConnectFailed { error: DbError::NoAnswer(CONNECT_GUARD), auth: false });
            return;
        }
    };
    let Opened { conn, wire, server, tracked, id } = opened;
    // The cancel logs in as the session does, without a database (one could be dropped since).
    let kill_opts = target.opts(None, &opts.application_name, &route);
    let killer = Killer::new(route, kill_opts, id);
    if let Ok(mut g) = canceller.killer.lock() {
        *g = Some(killer);
    }
    connected.store(true, Ordering::SeqCst);
    let _ = events.send(DbEvent::Connected);
    // The mode the session starts in; the metadata session's is what a new session of this
    // account starts in, which the app checks a tab's first run in.
    let _ = events.send(DbEvent::Language(Language::Sql(Dialect::MySql(tracked.mode(&server)))));
    // A session in a database of its own says where it works (it opened there: the server
    // refuses a database that does not exist or the user may not use).
    if let Some(db) = database.filter(|_| !opts.context.is_default()) {
        let _ = events.send(DbEvent::Context { database: db.clone(), schemas: vec![db] });
    }
    let link = Link::new(rx, wire);
    match role {
        SessionRole::Meta => crate::meta::meta_loop(conn, link, events, tracked).await,
        SessionRole::Query => {
            let killer = canceller.killer.lock().ok().and_then(|g| g.clone());
            let Some(killer) = killer else { return };
            let settings = crate::query::Settings { page_size: opts.page_size, read_only: opts.read_only };
            let (asked, busy) = (canceller.asked.clone(), canceller.busy.clone());
            crate::query::query_loop(conn, link, killer, asked, busy, events, settings, server, tracked, select_limit)
                .await
        }
    }
}

#[cfg(test)]
mod tests;
