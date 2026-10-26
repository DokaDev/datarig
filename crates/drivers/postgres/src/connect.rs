//! Opening sessions and test connections: profile -> `tokio_postgres::Config` and its TLS
//! ([`PgTls`]), auth error detection, error text. A session with a dialer
//! (`ConnectOptions::dialer`, a tunnel) reaches the server through a stream the dialer opens to
//! the profile's host and port (`Config::connect_raw`), and so do its cancel requests
//! (`CancelToken::cancel_query_raw`), each with the session's TLS.

use crate::link::Link;
use crate::meta::meta_loop;
use crate::query::query_loop;
use crate::route::{Cancel, Route, route};
use crate::tls::{PgTls, TlsError};
use datarig_core::driver::{
    Canceller, Capabilities, ConnectOptions, DbCommand, DbError, DbEvent, Driver, ExplainFormat, Hierarchy, PingError,
    PingInfo, Session, SessionRole, SessionStatement,
};
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::tls::{TlsProblem, TlsSettings};
use datarig_core::transport::DialerRef;
use futures::future::BoxFuture;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;
use tokio_postgres::config::{Host, SslNegotiation};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Config, SimpleQueryMessage};

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
            // Bound now to the request it is asked during; one that cannot be sent closes the
            // session, which says so (`Link::guard`).
            let request = cancel.unsent.request();
            tokio::spawn(async move { cancel.ask(request).await });
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
            language: Language::Sql(Dialect::Postgres),
            hierarchy: Hierarchy::DatabaseSchema,
            explain: Some(ExplainFormat::PostgresJson),
            reads_held_to_end: false,
            export: true,
            manual_commit: true,
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
                Ok((mut pgcfg, tls)) => {
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
                    run_session(pgcfg, tls, route, role, opts, events, rx, c2).await
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
            let (mut pgcfg, tls) = cfg.map_err(PingError::Failed)?;
            pgcfg.connect_timeout(timeout);
            let route = dialer.map(|d| route(&pgcfg, d)).transpose().map_err(PingError::Failed)?;
            let start = Instant::now();
            let attempt = async {
                let (client, _, _) = open(&pgcfg, &tls, route.as_ref()).await.map_err(|(e, _)| e)?;
                let row = client.query_typed_one("SHOW server_version", &[]).await.map_err(|e| db_error(&e))?;
                Ok::<_, DbError>(row.get::<_, String>(0))
            };
            match tokio::time::timeout(timeout, attempt).await {
                Ok(Ok(server_version)) => {
                    Ok(PingInfo { server_version, latency: start.elapsed(), encryption: tls.encryption() })
                }
                Ok(Err(e)) => Err(PingError::Failed(e)),
                Err(_) => Err(PingError::Timeout(timeout)),
            }
        })
    }
}

/// The profile's connection settings and its TLS. The TLS settings are datarig's own
/// (`ConnectionConfig::tls`): a DSN's TLS parameters are read there and left out of what
/// tokio-postgres parses. A connection to Unix sockets only is never encrypted (PostgreSQL has
/// no TLS on them; libpq ignores `sslmode` there too): only settings that cannot be read are an
/// error there. A server named by its address alone (`hostaddr`, no `host`) has the certificate
/// checked against that address, as the tunnel's route does. `sslnegotiation=direct` (a
/// handshake without the SSLRequest) is not supported.
fn pg_config(c: &ConnectionConfig) -> Result<(Config, PgTls), DbError> {
    let settings = c.tls();
    let mut cfg = if let Some(dsn) = &c.dsn {
        Config::from_str(&without_tls_params(dsn)).map_err(|e| DbError::Settings(Fault::other(chain(&e))))?
    } else {
        let mut cfg = Config::new();
        cfg.host(&c.host).port(c.port).user(&c.user).dbname(&c.database);
        if !c.password.is_empty() {
            cfg.password(&c.password);
        }
        cfg
    };
    if cfg.get_ssl_negotiation() == SslNegotiation::Direct {
        let why = "sslnegotiation=direct is not supported: TLS starts with an SSLRequest";
        return Err(DbError::Settings(Fault::other(why)));
    }
    let sockets_only = !cfg.get_hosts().is_empty()
        && cfg.get_hostaddrs().is_empty()
        && cfg.get_hosts().iter().all(|h| !matches!(h, Host::Tcp(_)));
    let tls = match (sockets_only, settings) {
        (_, Err(e @ (TlsProblem::UnknownMode(_) | TlsProblem::DsnUnreadable))) => return Err(DbError::Tls(e)),
        (true, _) => PgTls::new(TlsSettings::plain()),
        (false, settings) => PgTls::new(settings.map_err(DbError::Tls)?),
    };
    if cfg.get_hosts().is_empty() {
        for addr in cfg.get_hostaddrs().to_vec() {
            cfg.host(addr.to_string());
        }
    }
    cfg.ssl_mode(tls.ssl_mode());
    cfg.application_name("datarig").connect_timeout(Duration::from_secs(5));
    Ok((cfg, tls))
}

/// The TLS parameters a DSN may carry (datarig reads them itself).
const TLS_PARAMS: [&str; 4] = ["sslmode", "sslrootcert", "sslcert", "sslkey"];

/// A DSN without its TLS parameters, otherwise as written: a URL's query pairs (also of a URL
/// with several hosts), a `key=value` string's pairs.
fn without_tls_params(text: &str) -> String {
    use datarig_core::profile::dsn;
    if dsn::is_url(text) {
        let Some((head, query)) = text.trim().split_once('?') else { return text.to_string() };
        let decoded = |k: &str| dsn::url_params(k).and_then(|p| p.into_iter().next()).map(|(k, _)| k);
        let kept: Vec<&str> = query
            .split('&')
            .filter(|pair| {
                let key = pair.split_once('=').map_or(*pair, |(k, _)| k);
                !decoded(&format!("?{key}")).is_some_and(|k| TLS_PARAMS.contains(&k.as_str()))
            })
            .collect();
        return if kept.is_empty() { head.to_string() } else { format!("{head}?{}", kept.join("&")) };
    }
    let Some(pairs) = dsn::conninfo(text) else { return text.to_string() };
    let mut out = text.to_string();
    for p in pairs.iter().rev().filter(|p| TLS_PARAMS.contains(&p.key.as_str())) {
        out.replace_range(p.span.clone(), "");
    }
    out
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

/// A duration as a PostgreSQL setting in milliseconds takes it (at most what an `int` holds).
fn millis(d: Duration) -> u128 {
    d.as_millis().clamp(1, i32::MAX as u128)
}

/// `lock_timeout` for one transaction (gone at its end, so nothing stays on a pooled server
/// connection).
fn set_local_lock_timeout(wait: Duration) -> String {
    format!("SET LOCAL lock_timeout = {}", millis(wait))
}

/// Set the session's lock wait limit (`lock_wait`) and the server's idle limit
/// (`idle_session_timeout`, from version 14 on) for the session, in one round trip, only on a
/// direct connection: the server process the session talks to is the one whose id came in the
/// startup's `BackendKeyData` (a pooler sends an id of its own, and its server connections are
/// shared). Whether the connection is direct; a failed check reads as not (the limit then goes
/// in each transaction, which holds either way, and the idle limit is not set).
async fn apply_limits(client: &Client, lock_wait: Option<Duration>, server_idle: Option<Duration>) -> bool {
    let pid = client.backend_key_pid();
    let set = |name: &str, d: Option<Duration>, also: &str| match d {
        Some(d) => format!("CASE WHEN d.direct{also} THEN pg_catalog.set_config('{name}', '{}', false) END", millis(d)),
        None => "NULL".to_string(),
    };
    let ask = format!(
        "SELECT d.direct, {}, {} FROM (SELECT pg_catalog.pg_backend_pid() = {pid} AS direct) d",
        set("lock_timeout", lock_wait, ""),
        set(
            "idle_session_timeout",
            server_idle,
            " AND pg_catalog.current_setting('server_version_num')::int >= 140000"
        ),
    );
    match client.simple_query(&ask).await {
        Ok(msgs) => msgs.iter().any(|m| matches!(m, SimpleQueryMessage::Row(r) if r.get(0) == Some("t"))),
        Err(_) => false,
    }
}

/// The statements a query session starts with ([`ConnectOptions::session_sql`]): on a direct
/// connection each sent alone, for the session, in order; behind a pooler (`direct` false: not
/// known to be direct) none is set for the session, which would stay on a server connection
/// other clients share. There each one's `SET LOCAL` form goes first in every transaction
/// instead: returned (with the indexes of those that have none, which are not sent), each tried
/// once now in a transaction of its own (one simple query of two statements, which ends with
/// it), so a value the server refuses fails the connect here too. A statement the server refuses
/// fails the connect ([`DbError::SessionSql`]); the connection then closes with the task.
async fn start_up(
    client: &Client,
    sql: &[SessionStatement],
    direct: bool,
) -> Result<Option<(Vec<String>, Vec<usize>)>, DbError> {
    let failed = |index: usize, s: &SessionStatement, e: &tokio_postgres::Error| DbError::SessionSql {
        index,
        statement: s.sql.clone(),
        error: Box::new(db_error(e)),
    };
    if sql.is_empty() {
        return Ok(None);
    }
    if direct {
        for (i, s) in sql.iter().enumerate() {
            client.simple_query(&s.sql).await.map_err(|e| failed(i, s, &e))?;
        }
        return Ok(None);
    }
    let (mut each, mut refused) = (Vec::new(), Vec::new());
    for (i, s) in sql.iter().enumerate() {
        match &s.per_transaction {
            Some(local) => {
                client.simple_query(&format!("{local}; SELECT 1")).await.map_err(|e| failed(i, s, &e))?;
                each.push(local.clone());
            }
            None => refused.push(i),
        }
    }
    Ok(Some((each, refused)))
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

/// A connection's client, the receiver that resolves when it ends (with the reason) and the
/// handle that closes it (aborts the task that drives it).
type Opened = (Client, oneshot::Receiver<DbError>, AbortHandle);

/// Open a connection, directly or through `route`, with `tls`.
async fn open(cfg: &Config, tls: &PgTls, route: Option<&Route>) -> Result<Opened, (DbError, bool)> {
    let (done_tx, done_rx) = oneshot::channel();
    let fail = |e: tokio_postgres::Error| (db_error(&e), is_auth_error(&e));
    let (client, conn) = match route {
        None => {
            let (client, conn) = cfg.connect(tls.clone()).await.map_err(fail)?;
            (client, tokio::spawn(watch(conn, done_tx)))
        }
        Some(route) => {
            // `connect_timeout` covers the socket of a direct connect; here, the dial.
            let timeout = cfg.get_connect_timeout().copied().unwrap_or(CONNECT_GUARD);
            // The certificate names the profile's host, not the tunnel's end.
            let handshake = tls.to(route.name()).map_err(|e| (DbError::Tls(e.0), false))?;
            let stream = route.dial(timeout).await.map_err(|e| (e, false))?;
            let (client, conn) = cfg.connect_raw(stream, handshake).await.map_err(fail)?;
            (client, tokio::spawn(watch(conn, done_tx)))
        }
    };
    Ok((client, done_rx, conn.abort_handle()))
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
    tls: PgTls,
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
    let Ok(opened) = tokio::time::timeout(CONNECT_GUARD, open(&cfg, &tls, route.as_ref())).await else {
        let _ = events.send(DbEvent::ConnectFailed { error: DbError::NoAnswer(CONNECT_GUARD), auth: false });
        return;
    };
    let (client, ended, conn) = match opened {
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
    // The lock wait limit (query sessions only: the metadata session sets its own, shorter one in
    // each read) and the server's idle limit; whether the connection is direct is asked too when
    // a query session has start-up statements (they are set for the session only then).
    let lock_wait = opts.lock_wait.filter(|_| role == SessionRole::Query);
    let session_sql: &[SessionStatement] = match role {
        SessionRole::Query => &opts.session_sql,
        SessionRole::Meta => &[],
    };
    let direct = match lock_wait.is_some() || opts.server_idle.is_some() || !session_sql.is_empty() {
        true => apply_limits(&client, lock_wait, opts.server_idle).await,
        false => true,
    };
    let per_tx = match start_up(&client, session_sql, direct).await {
        Ok(per_tx) => per_tx,
        Err(error) => {
            let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
            return;
        }
    };
    if let Some((_, refused)) = &per_tx {
        let _ = events.send(DbEvent::SessionPerTransaction { refused: refused.clone() });
    }
    let _ = events.send(DbEvent::Encryption(tls.encryption()));
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
    match role {
        SessionRole::Meta => meta_loop(client, Link::new(rx, ended, conn, None), events).await,
        SessionRole::Query => {
            let cancel = Cancel::new(client.cancel_token(), route.clone(), tls.clone());
            let link = Link::new(rx, ended, conn, Some(cancel.unsent.clone()));
            if let Ok(mut g) = canceller.0.lock() {
                *g = Some(cancel.clone());
            }
            // Behind a pooler the lock wait limit goes in each transaction, as the search path does,
            // and the start-up statements after it.
            let lock = lock_wait.filter(|_| !direct).map(set_local_lock_timeout);
            let start = per_tx.map(|(each, _)| each).unwrap_or_default();
            let settings = crate::query::Settings {
                page_size: opts.page_size,
                read_only: opts.read_only,
                path_per_tx: path.is_some(),
                each_tx: path.into_iter().chain(lock).chain(start).collect(),
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
    if let Some(problem) = tls_problem(e) {
        return DbError::Tls(problem);
    }
    if let Some(db) = e.as_db_error() {
        let mut s = format!("{}: {}", db.severity(), db.message());
        if let Some(d) = db.detail() {
            s.push_str(&format!(" ({d})"));
        }
        // The server ended the session: its transaction sat idle past the limit.
        if db.code() == &SqlState::IDLE_IN_TRANSACTION_SESSION_TIMEOUT {
            return DbError::IdleInTransactionTimeout(s);
        }
        // The server ended the session: it sat idle past `idle_session_timeout`.
        if db.code() == &SqlState::IDLE_SESSION_TIMEOUT {
            return DbError::IdleSessionTimeout(s);
        }
        // A statement waited past `lock_timeout` (the server cancels it from its interrupt
        // check). `NOWAIT` and `SKIP LOCKED` fail with the same code elsewhere: they did not
        // wait.
        if db.code() == &SqlState::LOCK_NOT_AVAILABLE && db.routine() == Some("ProcessInterrupts") {
            return DbError::LockTimeout(s);
        }
        return DbError::Server(s);
    }
    if e.is_closed() {
        return DbError::Closed;
    }
    // The connection ended because the server started a COPY no request reads.
    if e.is_unexpected_copy() {
        return DbError::CopyStarted;
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

/// The TLS problem behind an error: the handshake's (or getting ready for it), a server that
/// answered it has no TLS when the mode needs it, or a TLS alert on the connection afterwards (a
/// server refusing the client certificate says so after a TLS 1.3 handshake, on the first read).
fn tls_problem(e: &tokio_postgres::Error) -> Option<TlsProblem> {
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        if let Some(t) = inner.downcast_ref::<TlsError>() {
            return Some(t.0.clone());
        }
        // tokio-postgres's own words when the server answers the SSLRequest with `N`.
        if inner.to_string() == "server does not support TLS" {
            return Some(TlsProblem::Unsupported);
        }
        if let Some(io) = inner.downcast_ref::<std::io::Error>() {
            return datarig_tls::problem_in_io(io, "");
        }
        src = inner.source();
    }
    None
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
