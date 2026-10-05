//! Opening sessions and test connections: profile -> `tokio_postgres::Config`, auth error
//! detection, error text. A session with a dialer (`ConnectOptions::dialer`, a tunnel) reaches
//! the server through a stream the dialer opens to the profile's host and port
//! (`Config::connect_raw`), and so do its cancel requests (`CancelToken::cancel_query_raw`).

use crate::link::Link;
use crate::meta::meta_loop;
use crate::query::query_loop;
use crate::route::{Cancel, Route, route};
use datarig_core::driver::{
    Canceller, Capabilities, ConnectOptions, DbCommand, DbError, DbEvent, Driver, PingError, PingInfo, Session,
    SessionRole,
};
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::profile::ConnectionConfig;
use datarig_core::transport::DialerRef;
use futures::future::BoxFuture;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Config, NoTls, SimpleQueryMessage};

pub struct PgDriver;

/// Upper bound for opening a session's connection.
pub(crate) const CONNECT_GUARD: Duration = Duration::from_secs(30);

/// Cancels the query session's statement: flags the request for the session, which stops a run
/// of several statements before its next one, and asks the server to cancel what runs now.
struct PgCanceller(Mutex<Option<Cancel>>, Arc<AtomicBool>);

impl Canceller for PgCanceller {
    fn cancel(&self) {
        self.1.store(true, Ordering::SeqCst);
        let cancel = self.0.lock().ok().and_then(|g| g.clone());
        if let Some(cancel) = cancel {
            tokio::spawn(async move {
                // Failure to deliver the cancel request surfaces as the query simply finishing.
                cancel.send().await;
            });
        }
    }
}

impl Driver for PgDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            server_paging: true,
            cancel: true,
            introspection: true,
            key_metadata: true,
            contexts: true,
            structure: true,
            ddl: true,
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
        let canceller = Arc::new(PgCanceller(Mutex::new(None), Arc::new(AtomicBool::new(false))));
        let cfg = cfg.clone();
        let c2 = canceller.clone();
        tokio::spawn(async move {
            match pg_config(&cfg) {
                Err(error) => {
                    let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
                }
                Ok(mut pgcfg) => {
                    pgcfg.application_name(&opts.application_name);
                    if opts.read_only {
                        read_only(&mut pgcfg);
                    }
                    if let Some(db) = &opts.context.database {
                        pgcfg.dbname(db);
                    }
                    if let Some(schema) = &opts.context.schema {
                        add_option(&mut pgcfg, &format!("-c search_path={}", option_escape(&search_path(schema))));
                    }
                    let route = match opts.dialer.clone().map(|d| route(&pgcfg, d)).transpose() {
                        Ok(route) => route,
                        Err(error) => {
                            let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
                            return;
                        }
                    };
                    run_session(pgcfg, route, role, opts, events, rx, c2).await
                }
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
        let cfg = pg_config(cfg);
        Box::pin(async move {
            let mut pgcfg = cfg.map_err(PingError::Failed)?;
            pgcfg.connect_timeout(timeout);
            let route = dialer.map(|d| route(&pgcfg, d)).transpose().map_err(PingError::Failed)?;
            let start = Instant::now();
            let attempt = async {
                let (client, _) = open(&pgcfg, route.as_ref()).await.map_err(|(e, _)| e)?;
                let row = client.query_typed_one("SHOW server_version", &[]).await.map_err(|e| db_error(&e))?;
                Ok::<_, DbError>(row.get::<_, String>(0))
            };
            match tokio::time::timeout(timeout, attempt).await {
                Ok(Ok(server_version)) => Ok(PingInfo { server_version, latency: start.elapsed() }),
                Ok(Err(e)) => Err(PingError::Failed(e)),
                Err(_) => Err(PingError::Timeout(timeout)),
            }
        })
    }
}

fn pg_config(c: &ConnectionConfig) -> Result<Config, DbError> {
    let mut cfg = if let Some(dsn) = &c.dsn {
        Config::from_str(dsn).map_err(|e| DbError::Settings(Fault::other(chain(&e))))?
    } else {
        let mut cfg = Config::new();
        cfg.host(&c.host).port(c.port).user(&c.user).dbname(&c.database);
        if !c.password.is_empty() {
            cfg.password(&c.password);
        }
        cfg.ssl_mode(match c.sslmode.as_str() {
            "disable" => SslMode::Disable,
            "require" | "verify-ca" | "verify-full" => SslMode::Require,
            _ => SslMode::Prefer,
        });
        cfg
    };
    cfg.application_name("datarig").connect_timeout(Duration::from_secs(5));
    Ok(cfg)
}

/// The startup setting that makes every transaction of the session read-only unless it asks
/// for `READ WRITE` (which the app refuses under a read-only policy). As a startup option it is
/// also what `RESET` and `DISCARD ALL` go back to. It goes after any options of a DSN, so it
/// wins over one that says otherwise. It is a second layer: the query session makes each of its
/// transactions `READ ONLY` itself, which holds even when a pooler drops the option or a
/// statement turns the default off.
const READ_ONLY_OPTION: &str = "-c default_transaction_read_only=on";

fn read_only(cfg: &mut Config) {
    add_option(cfg, READ_ONLY_OPTION);
}

/// Add a startup option after the ones the config has (a later `-c` of the same setting wins).
fn add_option(cfg: &mut Config, option: &str) {
    let options = match cfg.get_options().map(str::trim) {
        Some(o) if !o.is_empty() => format!("{o} {option}"),
        _ => option.to_string(),
    };
    cfg.options(options);
}

/// The `search_path` value of a session in `schema`: that schema, quoted as an identifier,
/// then `public`, or `"public"` alone when that is
/// the schema. `pg_catalog` stays implicitly first (a user's schema never shadows a built-in,
/// which the classifier's and the re-run allowlist's checks rely on) and `pg_temp` keeps its
/// implicit place.
pub(crate) fn search_path(schema: &str) -> String {
    let quoted = format!("\"{}\"", schema.replace('"', "\"\""));
    if schema == "public" { quoted } else { format!("{quoted}, public") }
}

/// What a session whose server dropped the startup option sends first in each transaction: the path for that transaction only, so nothing is left on a connection a
/// pooler hands to another client. A utility statement: it takes no snapshot, so `SET
/// TRANSACTION` still works after it.
pub(crate) fn set_local_path(schema: &str) -> String {
    format!("SET LOCAL search_path TO {}", search_path(schema))
}

/// `value` as one word of the startup `options` string: the server splits it at white space
/// and takes a backslash as "the next character literally".
pub(crate) fn option_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '\\' || c.is_whitespace() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Where a session in a context of its own works: its database, the schemas of its path that
/// exist, and, when the server dropped the startup option, the statement that sets the path in
/// each transaction ([`set_local_path`]).
struct Placed {
    database: String,
    schemas: Vec<String>,
    per_transaction: Option<String>,
}

/// Check that a session in `schema` has it as its search path, and say where it works. The
/// startup option sets it (so `RESET search_path` and `DISCARD ALL` return to it). A pooler may
/// drop startup options: then nothing is set for the session (behind a transaction pooler a
/// session setting stays on a server connection other clients get, and later transactions may
/// land on one without it); the query session sets it in each transaction instead, and where
/// it works is asked in such a transaction (one more round trip, on this path only).
///
/// The option applied only when the path is the one asked for **and** the client set it
/// (`pg_settings.source` is `client`): a pooled connection may carry the same value
/// from another client's `set_config(…, false)` (`session`), and a database's or role's default
/// may equal it (`database`, `user`); neither is this session's, so both take the path per
/// transaction. Asked in the same query (no round trip more).
async fn apply_context(client: &Client, schema: Option<&str>) -> Result<Placed, DbError> {
    let ask = "SELECT pg_catalog.current_setting('search_path'), pg_catalog.current_database()::text, \
               pg_catalog.current_schemas(false)::text[], \
               (SELECT s.source FROM pg_catalog.pg_settings s WHERE s.name = 'search_path')";
    let row = client.query_typed_one(ask, &[]).await.map_err(|e| db_error(&e))?;
    let placed = Placed { database: row.get(1), schemas: row.get(2), per_transaction: None };
    let applied = |s: &str| {
        row.get::<_, String>(0) == search_path(s) && row.get::<_, Option<String>>(3).as_deref() == Some("client")
    };
    let Some(schema) = schema.filter(|s| !applied(s)) else { return Ok(placed) };
    let set = set_local_path(schema);
    // One simple query is one implicit transaction: the `SET LOCAL` ends with it.
    let text = format!(
        "{set}; SELECT pg_catalog.current_database()::text; \
         SELECT pg_catalog.unnest(pg_catalog.current_schemas(false))::text"
    );
    let msgs = client.simple_query(&text).await.map_err(|e| db_error(&e))?;
    let (mut done, mut database, mut schemas) = (0, None, Vec::new());
    for m in msgs {
        match m {
            SimpleQueryMessage::CommandComplete(_) => done += 1,
            SimpleQueryMessage::Row(r) if done == 1 => database = r.get(0).map(str::to_string),
            SimpleQueryMessage::Row(r) => schemas.extend(r.get(0).map(str::to_string)),
            _ => {}
        }
    }
    Ok(Placed { database: database.unwrap_or(placed.database), schemas, per_transaction: Some(set) })
}

/// Whether the server made the session read-only (a pooler may drop startup options).
async fn is_read_only(client: &Client) -> bool {
    match client.simple_query("SHOW default_transaction_read_only").await {
        Ok(msgs) => msgs.iter().any(|m| matches!(m, SimpleQueryMessage::Row(r) if r.get(0) == Some("on"))),
        Err(_) => false,
    }
}

/// Whether a connect error means "wrong or missing password" (so a prompt can help).
fn is_auth_error(e: &tokio_postgres::Error) -> bool {
    if matches!(e.code(), Some(c) if *c == SqlState::INVALID_PASSWORD || *c == SqlState::INVALID_AUTHORIZATION_SPECIFICATION)
    {
        return true;
    }
    // tokio-postgres reports a password request without a configured password as a config
    // error whose `Display` doesn't include the underlying message (unlike its source chain),
    // e.g. a SCRAM server asking an unknown role for a password nobody supplied.
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        if inner.to_string().contains("password missing") {
            return true;
        }
        src = inner.source();
    }
    false
}

/// Open a connection, directly or through `route`. The receiver resolves when the connection
/// ends, with the reason.
async fn open(cfg: &Config, route: Option<&Route>) -> Result<(Client, oneshot::Receiver<DbError>), (DbError, bool)> {
    let (done_tx, done_rx) = oneshot::channel();
    let fail = |e: tokio_postgres::Error| (db_error(&e), is_auth_error(&e));
    let client = match route {
        None => {
            let (client, conn) = cfg.connect(NoTls).await.map_err(fail)?;
            tokio::spawn(watch(conn, done_tx));
            client
        }
        Some(route) => {
            // `connect_timeout` covers the socket of a direct connect; here, the dial.
            let timeout = cfg.get_connect_timeout().copied().unwrap_or(CONNECT_GUARD);
            let stream = route.dial(timeout).await.map_err(|e| (e, false))?;
            let (client, conn) = cfg.connect_raw(stream, NoTls).await.map_err(fail)?;
            tokio::spawn(watch(conn, done_tx));
            client
        }
    };
    Ok((client, done_rx))
}

/// Drive a connection until it ends and send why.
async fn watch<S, T>(conn: tokio_postgres::Connection<S, T>, done: oneshot::Sender<DbError>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let reason = match conn.await {
        Ok(()) => DbError::Closed,
        Err(e) => db_error(&e),
    };
    let _ = done.send(reason);
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    cfg: Config,
    route: Option<Route>,
    role: SessionRole,
    opts: ConnectOptions,
    events: UnboundedSender<DbEvent>,
    rx: UnboundedReceiver<DbCommand>,
    canceller: Arc<PgCanceller>,
) {
    // `connect_timeout` only covers the socket; a server that accepts TCP but never answers
    // would otherwise keep this task alive forever (the UI gives up earlier, see
    // `app::CONNECT_TIMEOUT`).
    let Ok(opened) = tokio::time::timeout(CONNECT_GUARD, open(&cfg, route.as_ref())).await else {
        let _ = events.send(DbEvent::ConnectFailed { error: DbError::NoAnswer(CONNECT_GUARD), auth: false });
        return;
    };
    let (client, ended) = match opened {
        Ok(c) => c,
        Err((error, auth)) => {
            let _ = events.send(DbEvent::ConnectFailed { error, auth });
            return;
        }
    };
    // A pooler may drop startup options, so the session's default may not be read-only. The
    // query session does not depend on it (every transaction it opens is `READ ONLY`, see
    // `query`), and the metadata session runs only the app's own catalog reads: the session
    // opens, and the UI is told the server ignored the option.
    let default_ignored = opts.read_only && !is_read_only(&client).await;
    // A session in a context of its own checks it and says where it works; one with the
    // profile's defaults costs nothing more (no round trip).
    let context = match opts.context.is_default() {
        true => None,
        false => match apply_context(&client, opts.context.schema.as_deref()).await {
            Ok(c) => Some(c),
            Err(error) => {
                let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
                return;
            }
        },
    };
    let _ = events.send(DbEvent::Connected);
    if default_ignored {
        let _ = events.send(DbEvent::ReadOnlyPerTransaction);
    }
    let mut path = None;
    if let Some(Placed { database, schemas, per_transaction }) = context {
        // Only the query session sends what the user runs; the metadata session's reads name
        // their schemas.
        if per_transaction.is_some() && role == SessionRole::Query {
            let _ = events.send(DbEvent::ContextPerTransaction);
            path = per_transaction;
        }
        let _ = events.send(DbEvent::Context { database, schemas });
    }
    let link = Link::new(rx, ended);
    match role {
        SessionRole::Meta => meta_loop(client, link, events).await,
        SessionRole::Query => {
            let cancel = Cancel { token: client.cancel_token(), route: route.clone() };
            if let Ok(mut g) = canceller.0.lock() {
                *g = Some(cancel.clone());
            }
            let settings = crate::query::Settings {
                page_size: opts.page_size,
                read_only: opts.read_only,
                path,
                statement_cache: opts.statement_cache,
            };
            query_loop(client, link, cancel, canceller.1.clone(), events, settings).await
        }
    }
}

/// A driver error as a [`DbError`]: the server's own message as it is, anything else as a
/// fault (a host name that did not resolve, else the io kind when the cause is an io error;
/// the whole chain as its detail).
pub(crate) fn db_error(e: &tokio_postgres::Error) -> DbError {
    if let Some(db) = e.as_db_error() {
        let mut s = format!("{}: {}", db.severity(), db.message());
        if let Some(d) = db.detail() {
            s.push_str(&format!(" ({d})"));
        }
        return DbError::Server(s);
    }
    if e.is_closed() {
        return DbError::Closed;
    }
    let mut kind = FaultKind::Other;
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        if let Some(io) = inner.downcast_ref::<std::io::Error>() {
            kind = if lookup_failed(io) { FaultKind::HostNotFound } else { FaultKind::Io(io.kind()) };
            break;
        }
        src = inner.source();
    }
    DbError::Connection(Fault::new(kind, chain(e)))
}

/// Whether `io` is a failed name lookup (getaddrinfo). The standard library reports one only in
/// words on Unix, and on Windows as the lookup's own Windows Sockets code: WSAHOST_NOT_FOUND,
/// WSATRY_AGAIN, WSANO_RECOVERY or WSANO_DATA (11001 to 11004), which nothing but a lookup
/// returns.
fn lookup_failed(io: &std::io::Error) -> bool {
    if cfg!(windows) {
        matches!(io.raw_os_error(), Some(11001..=11004))
    } else {
        io.to_string().starts_with("failed to lookup address information")
    }
}

/// An error and its sources, as one line (for the error log).
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        s.push_str(&format!(": {inner}"));
        src = inner.source();
    }
    s
}

pub(crate) fn is_cancel(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::QUERY_CANCELED)
}

#[cfg(test)]
mod tests;
