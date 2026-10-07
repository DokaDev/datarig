//! Opening sessions and test connections: profile -> the server, its credentials and the
//! session's settings, through the session's route (directly or a tunnel's dialer), and the
//! canceller (`KILL QUERY` on a connection of its own).

use crate::link::{Link, Next};
use crate::route::Route;
use crate::session::{self, CONNECT_GUARD, Killer, Opened, Settings, Target, my_error};
use datarig_core::driver::{
    Canceller, Capabilities, ConnectOptions, DbCommand, DbError, DbEvent, Driver, Hierarchy, PingError, PingInfo,
    Session, SessionRole,
};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::transport::DialerRef;
use futures::future::BoxFuture;
use mysql_async::prelude::Queryable;
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
            tokio::spawn(async move { killer.kill_query().await });
        }
    }
}

impl Driver for MyDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            server_paging: false,
            cancel: true,
            introspection: false,
            key_metadata: false,
            contexts: false,
            structure: false,
            ddl: false,
            language: Language::Sql(Dialect::MySql(MySqlMode::default())),
            hierarchy: Hierarchy::SchemaOnly,
            explain: None,
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
        let target = Target::of(cfg);
        let c2 = canceller.clone();
        tokio::spawn(async move {
            match target {
                Err(error) => {
                    let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
                }
                Ok(target) => run_session(target, role, opts, events, rx, c2).await,
            }
        });
        Session::new(self.capabilities(), role, tx, canceller)
    }

    fn ping(
        &self,
        cfg: &ConnectionConfig,
        timeout: Duration,
        dialer: Option<DialerRef>,
    ) -> BoxFuture<'static, Result<PingInfo, PingError>> {
        let target = Target::of(cfg);
        Box::pin(async move {
            let target = target.map_err(PingError::Failed)?;
            let route = Route::new(&target.host, target.port, dialer);
            let opts = target.opts(target.database.as_deref(), "datarig-test");
            let start = Instant::now();
            let attempt = async {
                let stream = route.dial(timeout).await?;
                let (_wire, stream) = crate::wire::Wire::new(stream);
                let mut conn =
                    mysql_async::Conn::connect_with_stream(opts, Box::new(stream)).await.map_err(|e| my_error(&e))?;
                let server = session::Server { version: conn.server_version(), mariadb: conn.is_mariadb() };
                let version: Result<Option<String>, _> = match server.check() {
                    Ok(()) => conn.query_first("SELECT VERSION()").await.map_err(|e| my_error(&e)),
                    Err(e) => Err(e),
                };
                let _ = conn.disconnect().await;
                Ok::<_, DbError>(version?.unwrap_or_else(|| server.label()))
            };
            match tokio::time::timeout(timeout, attempt).await {
                Ok(Ok(server_version)) => Ok(PingInfo { server_version, latency: start.elapsed() }),
                Ok(Err(e)) => Err(PingError::Failed(e)),
                Err(_) => Err(PingError::Timeout(timeout)),
            }
        })
    }
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
) {
    let route = Route::new(&target.host, target.port, opts.dialer.clone());
    let database = start_database(&target, &opts);
    let my_opts = target.opts(database.as_deref(), &opts.application_name);
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
    let Opened { conn, wire, server, tracked } = opened;
    // The cancel logs in as the session does, without a database (one could be dropped since).
    let killer = Killer::new(route, target.opts(None, &opts.application_name), conn.id());
    if let Ok(mut g) = canceller.killer.lock() {
        *g = Some(killer);
    }
    let _ = events.send(DbEvent::Connected);
    if role == SessionRole::Query {
        let _ = events.send(DbEvent::Language(Language::Sql(Dialect::MySql(tracked.mode(&server)))));
    }
    let link = Link::new(rx, wire);
    serve(conn, link, events).await;
}

/// Answer every command as not supported, until the session closes or its connection ends.
async fn serve(conn: mysql_async::Conn, mut link: Link, events: UnboundedSender<DbEvent>) {
    loop {
        let ev = match link.next().await {
            Next::Command(DbCommand::LoadSchemas) => DbEvent::Schemas(Err(DbError::NotSupported)),
            Next::Command(DbCommand::LoadObjects { schema }) => {
                DbEvent::Objects { schema, result: Err(DbError::NotSupported) }
            }
            Next::Command(DbCommand::LoadCatalog) => DbEvent::Catalog(Err(DbError::NotSupported)),
            Next::Command(DbCommand::LoadKeys) => DbEvent::Keys(Err(DbError::NotSupported)),
            Next::Command(DbCommand::LoadDatabases) => DbEvent::Databases(Err(DbError::NotSupported)),
            Next::Command(DbCommand::LoadStructure { schema, table }) => {
                DbEvent::Structure { schema, table, result: Err(DbError::NotSupported) }
            }
            Next::Command(DbCommand::LoadDdl { id, .. }) => DbEvent::Ddl { id, result: Err(DbError::NotSupported) },
            Next::Command(DbCommand::Execute { id, .. } | DbCommand::Resume { id, .. }) => {
                DbEvent::Failed { id, error: DbError::NotSupported, cancelled: false }
            }
            // Nothing is ever held open.
            Next::Command(DbCommand::ClosePortal { .. }) => continue,
            Next::Command(DbCommand::FetchMore { id }) => {
                DbEvent::Page { id, columns: None, rows: Vec::new(), more: false, elapsed: Duration::ZERO }
            }
            Next::Command(DbCommand::Count { id, .. }) => {
                DbEvent::Counted { id, result: Err(DbError::NotSupported), snapshot: false }
            }
            Next::Command(DbCommand::CheckRepeat { id, .. }) => {
                DbEvent::RepeatChecked { id, result: Err(DbError::NotSupported) }
            }
            Next::Closed => {
                let _ = conn.disconnect().await;
                return;
            }
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        if events.send(ev).is_err() {
            return;
        }
    }
}
