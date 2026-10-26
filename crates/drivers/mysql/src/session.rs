//! Opening a connection: the profile's server and credentials ([`Target`]), the handshake over
//! a stream of the session's route, the server's version ([`Server`]), and the session's
//! settings in one statement ([`init_sql`]); the session state the server reports
//! ([`Tracked`]); and the cancel's connection of its own ([`Killer`]).

use crate::route::{Route, io_fault};
use crate::wire::{TlsFailed, Wire};
use datarig_core::driver::{CancelFailure, DbError, SessionRole, SessionValues};
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::dsn::{self, Scheme};
use datarig_core::sql::dialect::MySqlMode;
use datarig_core::tls::{Encryption, TlsProblem, TlsSettings};
use datarig_core::transport::{BoxedStream, DialerRef};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SessionStateChange, StreamTls, UpgradeFuture};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Upper bound for opening a session's connection (the stream, the login, the settings).
pub(crate) const CONNECT_GUARD: Duration = Duration::from_secs(30);

/// How long a connection's stream may take to open (a direct connection's TCP connect, or the
/// dial through a tunnel).
pub(crate) const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a cancel's connection may take to open and send its `KILL QUERY`.
pub(crate) const KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// The server and credentials of a profile, from its fields or its `mysql://` URL.
#[derive(Clone)]
pub(crate) struct Target {
    pub(crate) host: String,
    pub(crate) port: u16,
    user: String,
    password: String,
    /// The profile's database; `None`: none (the session starts without a current database).
    pub(crate) database: Option<String>,
    /// The server's public key (PEM) from the profile's key file.
    server_key: Option<Vec<u8>>,
    /// The profile lets a direct login to another machine ask the server for its key.
    allow_key_retrieval: bool,
    /// The profile's TLS settings.
    tls: TlsSettings,
}

impl Target {
    /// The target of `cfg`, with its server public key file read.
    pub(crate) fn of(cfg: &ConnectionConfig) -> Result<Self, DbError> {
        let settings = |why: String| DbError::Settings(Fault::other(why));
        let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let server_key = cfg.server_public_key_file.as_deref().map(crate::server_key::read).transpose()?;
        let tls = cfg.tls().map_err(DbError::Tls)?;
        match &cfg.dsn {
            Some(text) => {
                // The parser's error may quote a piece of the URL, a password among them.
                let d = dsn::parse(text).map_err(|_| settings("not a mysql:// URL".to_string()))?;
                if d.scheme != Scheme::MySql {
                    return Err(settings("not a mysql:// URL".to_string()));
                }
                if let Some((k, _)) = d.params.first() {
                    return Err(settings(format!("a mysql:// URL takes no parameters (it has {k})")));
                }
                // The password the app resolved for the profile, else the URL's own.
                let password =
                    if cfg.password.is_empty() { d.password.unwrap_or_default() } else { cfg.password.clone() };
                let host = if d.host.is_empty() { "localhost".to_string() } else { d.host };
                Ok(Self {
                    host,
                    port: d.port.unwrap_or(Scheme::MySql.default_port()),
                    user: d.user,
                    password,
                    database: some(&d.database),
                    server_key,
                    allow_key_retrieval: cfg.allow_public_key_retrieval,
                    tls,
                })
            }
            None => Ok(Self {
                host: cfg.host.clone(),
                port: cfg.port,
                user: cfg.user.clone(),
                password: cfg.password.clone(),
                database: some(&cfg.database),
                server_key,
                allow_key_retrieval: cfg.allow_public_key_retrieval,
                tls,
            }),
        }
    }

    /// The route to the server: directly or through `dialer`, with the profile's TLS.
    pub(crate) fn route(&self, dialer: Option<DialerRef>) -> Result<Route, DbError> {
        Route::new(&self.host, self.port, dialer).with_tls(&self.tls, self.allow_key_retrieval)
    }

    /// The client's options for a connection over `route` to `database` (or none) that the
    /// server lists as `program_name` (`performance_schema.session_connect_attrs`). mysql_async
    /// asks the server nothing more after the login (`max_allowed_packet` and `wait_timeout` are
    /// given), never moves to the server's socket file, and never sends the password as it is
    /// over an unencrypted stream: the cleartext method is off, a full `caching_sha2_password`
    /// login over TLS sends it inside TLS only where the route allows it ([`Route::with_tls`]),
    /// and one over a stream that is not encrypted sends it encrypted with the server's public
    /// key. That is
    /// the profile's key file (`--server-public-key-path`), else the key the server sends when
    /// asked (`--get-server-public-key`), over the same connection: on a direct connection to
    /// another machine whoever sits in between could send theirs, so it is asked only when the
    /// profile allows it (an SSH tunnel's channel is encrypted end to end to its bastion, and a
    /// loopback connection never leaves the machine).
    pub(crate) fn opts(&self, database: Option<&str>, program: &str, route: &Route) -> Opts {
        let attrs = std::collections::HashMap::from([("program_name".to_string(), program.to_string())]);
        OptsBuilder::default()
            .ip_or_hostname(self.host.clone())
            .tcp_port(self.port)
            .user(Some(self.user.clone()))
            .pass((!self.password.is_empty()).then(|| self.password.clone()))
            .db_name(database.map(str::to_string))
            .prefer_socket(false)
            .max_allowed_packet(Some(MAX_PACKET))
            .wait_timeout(Some(28_800))
            .enable_cleartext_plugin(false)
            .server_public_key(self.server_key.clone())
            .public_key_retrieval(self.allow_key_retrieval || !route.exposed())
            .connect_attributes(attrs)
            .into()
    }
}

/// The largest packet the client reads: MySQL's own limit (a row of a result that is bigger
/// than the server's `max_allowed_packet` never comes).
const MAX_PACKET: usize = 1 << 30;

/// The server, as its handshake says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Server {
    pub(crate) version: (u16, u16, u16),
    pub(crate) mariadb: bool,
}

/// The oldest MySQL the driver works with.
const MYSQL_MIN: (u16, u16, u16) = (8, 0, 0);

/// The oldest MariaDB the driver tries (a best effort).
const MARIADB_MIN: (u16, u16, u16) = (10, 6, 0);

impl Server {
    /// `MySQL 8.4.6`, `MariaDB 11.4.2`.
    pub(crate) fn label(&self) -> String {
        let (a, b, c) = self.version;
        format!("{} {a}.{b}.{c}", if self.mariadb { "MariaDB" } else { "MySQL" })
    }

    /// Refused when it is older than the driver supports (or says no version it can read).
    pub(crate) fn check(&self) -> Result<(), DbError> {
        let (min, name) = if self.mariadb { (MARIADB_MIN, "MariaDB") } else { (MYSQL_MIN, "MySQL") };
        if self.version >= min {
            return Ok(());
        }
        let server = if self.version == (0, 0, 0) { format!("{name} (unknown version)") } else { self.label() };
        Err(DbError::VersionUnsupported { server, needed: format!("{name} {}.{}", min.0, min.1) })
    }

    /// Whether the server reads `$$…$$` as a quote: MySQL's client asks with `select $$`,
    /// which MySQL 8.4 and 9.x refuse as a syntax error (8.0 reads `$$` as a name), and
    /// MariaDB reads as a name.
    pub(crate) fn dollar_quotes(&self) -> bool {
        !self.mariadb && self.version >= (8, 1, 0)
    }

    /// The session variable that makes its transactions read-only (`tx_read_only` before
    /// MariaDB 11.1).
    pub(crate) fn read_only_var(&self) -> &'static str {
        if self.mariadb && self.version < (11, 1, 0) { "tx_read_only" } else { "transaction_read_only" }
    }

    /// The `SET` assignment that stops a `SELECT` after `ms` milliseconds.
    fn timeout(&self, ms: u64) -> String {
        if self.mariadb {
            format!("SESSION max_statement_time = {}", ms as f64 / 1000.0)
        } else {
            format!("SESSION max_execution_time = {ms}")
        }
    }
}

/// How the session is set up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settings {
    pub(crate) role: SessionRole,
    /// The profile's policy is read-only: the session's transactions are read-only on the
    /// server.
    pub(crate) read_only: bool,
    /// The query session's `sql_select_limit` (a page and one row more), `None` for none.
    pub(crate) select_limit: Option<u64>,
    /// The query session's lock wait limit in seconds (`lock_wait_timeout` and
    /// `innodb_lock_wait_timeout`), `None` for the server's.
    pub(crate) lock_wait: Option<u64>,
    /// The session's `wait_timeout` in seconds (the server's idle limit), `None` for the
    /// server's.
    pub(crate) wait_timeout: Option<u64>,
}

/// A duration in whole seconds as the servers' settings take it: rounded up, at least one.
pub(crate) fn whole_secs(d: Duration) -> u64 {
    d.as_secs().saturating_add(u64::from(d.subsec_nanos() > 0)).max(1)
}

/// How long a lookup of the metadata session waits for a metadata lock another session holds
/// or waits for (`lock_wait_timeout`, seconds): it gives up instead of queueing.
pub(crate) const META_LOCK_WAIT: u32 = 2;

/// How long a `SELECT` of the metadata session may run, in milliseconds.
pub(crate) const META_TIMEOUT_MS: u64 = 10_000;

/// The session's settings, as one `SET` (one round trip): the client character set `utf8mb4`
/// (the server's default collation for it), the session state the server reports in its OK
/// packets (the sql mode, read-only and `sql_select_limit`, the current database), the sql
/// mode assigned to itself so the server reports it at once, and by role:
///
/// * the query session: `sql_select_limit` (results stop after a page and one row more, so a
///   statement ends, and with it every metadata lock it took), and the profile's lock wait
///   limit, if any, for both kinds of locks (a metadata lock's `lock_wait_timeout`, a row lock's
///   `innodb_lock_wait_timeout`);
/// * every session: the server's idle limit, if one is given (`wait_timeout`);
/// * the metadata session: a lookup waits at most [`META_LOCK_WAIT`] seconds for a metadata
///   lock, a `SELECT` runs at most [`META_TIMEOUT_MS`], and the session is read-only (it only
///   reads the catalog);
/// * a read-only profile's sessions: read-only.
pub(crate) fn init_sql(server: &Server, s: &Settings) -> String {
    let ro = server.read_only_var();
    let mut parts = vec![
        "NAMES utf8mb4".to_string(),
        format!(
            "SESSION session_track_system_variables = \
             'sql_mode,{ro},sql_select_limit,time_zone,character_set_results,character_set_connection'"
        ),
        "SESSION session_track_schema = ON".to_string(),
        "SESSION sql_mode = @@SESSION.sql_mode".to_string(),
        // Reported back as they are: the zone the session shows a `TIMESTAMP` in, and the
        // character sets of its text (set by `NAMES` before the reports were asked for).
        "SESSION time_zone = @@SESSION.time_zone".to_string(),
        "SESSION character_set_results = @@SESSION.character_set_results".to_string(),
        "SESSION character_set_connection = @@SESSION.character_set_connection".to_string(),
        // Each statement outside a transaction commits on its own, whatever the server's default
        // (`init_connect`, a global `autocommit = 0`): the session follows the flag from here.
        "SESSION autocommit = 1".to_string(),
    ];
    if let Some(n) = s.select_limit {
        parts.push(format!("SESSION sql_select_limit = {n}"));
    }
    if let Some(n) = s.lock_wait.filter(|_| s.role == SessionRole::Query) {
        parts.push(format!("SESSION lock_wait_timeout = {n}"));
        parts.push(format!("SESSION innodb_lock_wait_timeout = {n}"));
    }
    if let Some(n) = s.wait_timeout {
        parts.push(format!("SESSION wait_timeout = {n}"));
    }
    if s.role == SessionRole::Meta {
        parts.push(format!("SESSION lock_wait_timeout = {META_LOCK_WAIT}"));
        parts.push(server.timeout(META_TIMEOUT_MS));
        // `SHOW CREATE …` quotes every name, whatever the server's default.
        parts.push("SESSION sql_quote_show_create = 1".to_string());
    }
    if s.read_only || s.role == SessionRole::Meta {
        parts.push(format!("SESSION {ro} = ON"));
    }
    format!("SET {}", parts.join(", "))
}

/// What a statement that ran code of the server's may have changed in the session without
/// reporting it, asked in one statement: the sql mode, the time zone and the character sets.
pub(crate) const ASK_VALUES: &str = "SELECT @@SESSION.sql_mode, @@SESSION.time_zone, \
     IFNULL(@@SESSION.character_set_results, ''), @@SESSION.character_set_connection";

/// What the server reported of the session (its session state tracking), as far as the driver
/// follows it; `None`: not reported yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tracked {
    pub(crate) sql_mode: Option<String>,
    pub(crate) read_only: Option<bool>,
    pub(crate) select_limit: Option<String>,
    /// The current database (`USE`, `COM_INIT_DB`); `Some("")` for none.
    pub(crate) schema: Option<String>,
    /// `time_zone`.
    pub(crate) time_zone: Option<String>,
    /// `character_set_results` and `character_set_connection` (`utf8mb4`, which `SET NAMES`
    /// at connect sets, until the server reports another).
    pub(crate) results_charset: Option<String>,
    pub(crate) connection_charset: Option<String>,
}

impl Tracked {
    /// Take what the last OK packet of `conn` reports.
    pub(crate) fn update(&mut self, conn: &Conn) {
        let Some(ok) = conn.last_ok_packet() else { return };
        let Ok(changes) = ok.session_state_info() else { return };
        for change in changes {
            let Ok(change) = change.decode() else { continue };
            match change {
                SessionStateChange::SystemVariables(vars) => {
                    for v in vars {
                        let (name, value) = (v.name_str().to_ascii_lowercase(), v.value_str().into_owned());
                        match name.as_str() {
                            "sql_mode" => self.sql_mode = Some(value),
                            "transaction_read_only" | "tx_read_only" => self.read_only = Some(on(&value)),
                            "sql_select_limit" => self.select_limit = Some(value),
                            "time_zone" => self.time_zone = Some(value),
                            "character_set_results" => self.results_charset = Some(value),
                            "character_set_connection" => self.connection_charset = Some(value),
                            _ => {}
                        }
                    }
                }
                SessionStateChange::Schema(s) => self.schema = Some(s.as_str().into_owned()),
                _ => {}
            }
        }
    }

    /// What the session's values are read in: its time zone and the character set of its
    /// results' text (that of the connection when that one is not `utf8mb4`; `binary` for no
    /// conversion, `character_set_results = NULL`). `None` until the server reported all three:
    /// values it never said are not known.
    pub(crate) fn values(&self) -> Option<SessionValues> {
        let named = |cs: &str| match cs {
            "" | "NULL" => "binary".to_string(),
            cs => cs.to_string(),
        };
        let results = named(self.results_charset.as_deref()?);
        let connection = named(self.connection_charset.as_deref()?);
        let charset = if results == "utf8mb4" && connection != "utf8mb4" { connection } else { results };
        Some(SessionValues { time_zone: self.time_zone.clone()?, charset })
    }

    /// What [`ASK_VALUES`] answered.
    pub(crate) fn set_asked(&mut self, (mode, zone, results, connection): (String, String, String, String)) {
        self.sql_mode = Some(mode);
        self.time_zone = Some(zone);
        self.results_charset = Some(results);
        self.connection_charset = Some(connection);
    }

    /// The session's values are not known any more: reported as `unknown`, which no copy takes
    /// for a fixed time zone or for UTF-8.
    pub(crate) fn forget_values(&mut self) {
        let unknown = || Some("unknown".to_string());
        (self.time_zone, self.results_charset, self.connection_charset) = (unknown(), unknown(), unknown());
    }

    /// The mode the session's text is read in.
    pub(crate) fn mode(&self, server: &Server) -> MySqlMode {
        let flags: Vec<&str> = self.sql_mode.as_deref().unwrap_or("").split(',').map(str::trim).collect();
        MySqlMode {
            ansi_quotes: flags.contains(&"ANSI_QUOTES"),
            no_backslash_escapes: flags.contains(&"NO_BACKSLASH_ESCAPES"),
            dollar_quotes: server.dollar_quotes(),
        }
    }
}

/// A boolean setting's value as the server reports it (`ON`, `1`).
fn on(value: &str) -> bool {
    value.eq_ignore_ascii_case("on") || value == "1"
}

/// A connection that is ready for the session.
pub(crate) struct Opened {
    pub(crate) conn: Conn,
    pub(crate) wire: Wire,
    /// How its traffic goes.
    pub(crate) encryption: Encryption,
    pub(crate) server: Server,
    pub(crate) tracked: Tracked,
    /// The connection's id on the server (what `KILL QUERY` names).
    pub(crate) id: u64,
}

/// Open a connection to `target` over a stream of `route` with `opts`, check the server's
/// version and set the session up ([`init_sql`]); the session's read-only is checked with the
/// server. A failure says whether it is about the password (`true`: a prompt can help).
///
/// A query session then runs its start-up statements (`session_sql`, each alone and in order,
/// [`datarig_core::driver::ConnectOptions::session_sql`]) before the read-only check, which so
/// also catches one that turned read-only off; a statement the server refuses fails the open
/// ([`DbError::SessionSql`]).
pub(crate) async fn open(
    route: &Route,
    opts: Opts,
    settings: Settings,
    session_sql: &[String],
) -> Result<Opened, (DbError, bool)> {
    let stream = route.dial(DIAL_TIMEOUT).await.map_err(|e| (e, false))?;
    let (wire, conn, encryption) = login(route, stream, opts).await;
    let mut conn = conn.map_err(|e| connect_error(&e))?;
    let server = Server { version: conn.server_version(), mariadb: conn.is_mariadb() };
    if let Err(e) = server.check() {
        quit(conn).await;
        return Err((e, false));
    }
    let mut tracked = Tracked::default();
    let set = conn.query_drop(init_sql(&server, &settings)).await;
    set.map_err(|e| (my_error(&e), false))?;
    tracked.update(&conn);
    for (index, sql) in session_sql.iter().enumerate() {
        if let Err(e) = conn.query_drop(sql.as_str()).await {
            let error = DbError::SessionSql { index, statement: sql.clone(), error: Box::new(my_error(&e)) };
            quit(conn).await;
            return Err((error, false));
        }
        tracked.update(&conn);
    }
    let read_only = settings.read_only || settings.role == SessionRole::Meta;
    // A server (or a proxy in front of it) that did not report them is asked.
    if tracked.sql_mode.is_none() || tracked.values().is_none() || (read_only && tracked.read_only.is_none()) {
        let ro = server.read_only_var();
        let ask = format!(
            "SELECT @@SESSION.sql_mode, CAST(@@SESSION.{ro} AS CHAR), @@SESSION.time_zone, \
             IFNULL(@@SESSION.character_set_results, ''), @@SESSION.character_set_connection"
        );
        let row: Option<(String, String, String, String, String)> =
            conn.query_first(ask).await.map_err(|e| (my_error(&e), false))?;
        if let Some((mode, ro, zone, results, connection)) = row {
            tracked.sql_mode = Some(mode);
            tracked.read_only = Some(on(&ro));
            tracked.time_zone = Some(zone);
            tracked.results_charset = Some(results);
            tracked.connection_charset = Some(connection);
        }
    }
    if read_only && tracked.read_only != Some(true) {
        quit(conn).await;
        return Err((DbError::Connection(Fault::other("the server did not make the session read-only")), false));
    }
    // The handshake has the id's lower 32 bits: MySQL's ids have no more, MariaDB's may.
    let id = match server.mariadb {
        false => u64::from(conn.id()),
        true => {
            let id: Option<u64> =
                conn.query_first("SELECT CONNECTION_ID()").await.map_err(|e| (my_error(&e), false))?;
            id.unwrap_or(u64::from(conn.id()))
        }
    };
    Ok(Opened { conn, wire, encryption, server, tracked, id })
}

/// The handshake and login over `stream` of `route`, through a [`Wire`]: encrypted as the
/// route's TLS says (the wire carries TLS from the SSLRequest on, [`Wire::encrypt`]). How its
/// traffic went, once logged in.
pub(crate) async fn login(
    route: &Route,
    stream: BoxedStream,
    opts: Opts,
) -> (Wire, Result<Conn, mysql_async::Error>, Encryption) {
    let (wire, stream) = Wire::new(stream);
    let outcome: Arc<std::sync::Mutex<Option<Encryption>>> = Arc::default();
    let tls = route.tls.connector.clone().map(|connector| {
        let (wire, outcome) = (wire.clone(), outcome.clone());
        StreamTls {
            required: route.tls.required,
            password_ok: route.tls.password_ok,
            upgrade: Box::new(move |stream| -> UpgradeFuture {
                Box::pin(async move {
                    let encryption = wire.encrypt(&connector).await?;
                    if let Ok(mut o) = outcome.lock() {
                        *o = Some(encryption);
                    }
                    Ok(stream)
                })
            }),
        }
    });
    let conn = Conn::connect_with_stream_tls(opts, Box::new(stream), tls).await;
    let encryption = outcome.lock().ok().and_then(|o| *o).unwrap_or(Encryption::Plain);
    (wire, conn, encryption)
}

/// How long closing a connection waits for the server to take its `COM_QUIT`.
const QUIT_WAIT: Duration = Duration::from_secs(2);

/// Close `conn` (`COM_QUIT`), waiting at most [`QUIT_WAIT`]: a stalled stream does not keep the
/// task.
pub(crate) async fn quit(conn: Conn) {
    let _ = tokio::time::timeout(QUIT_WAIT, conn.disconnect()).await;
}

/// A failure while the connection opens, and whether it is about the password.
pub(crate) fn connect_error(e: &mysql_async::Error) -> (DbError, bool) {
    match e {
        // ER_ACCESS_DENIED_ERROR: a wrong or missing password (or user), or an account that
        // requires TLS.
        mysql_async::Error::Server(s) if s.code == 1045 => {
            (DbError::AccessDenied(format!("ERROR {} ({}): {}", s.code, s.state, s.message)), true)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::PublicKeyRetrievalDisabled) => {
            (DbError::KeyRetrievalRefused, false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::NoClientSslFlagFromServer) => {
            (DbError::Tls(TlsProblem::Unsupported), false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::PasswordOverUnverifiedTls) => {
            (DbError::Tls(TlsProblem::PasswordUnverified), false)
        }
        // A TLS alert while logging in (a server that refuses the client certificate says so
        // after a TLS 1.3 handshake, on the first read).
        mysql_async::Error::Io(mysql_async::IoError::Io(io)) if datarig_tls::problem_in_io(io, "").is_some() => {
            (DbError::Tls(datarig_tls::problem_in_io(io, "").expect("checked")), false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::PasswordTooLongForKey { max }) => {
            (DbError::PasswordTooLong { max: *max }, false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::InvalidServerPublicKey) => {
            (DbError::ServerKeyInvalid, false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::UnknownAuthPlugin { name }) => {
            (DbError::AuthUnsupported(name.clone()), false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::CleartextPluginDisabled) => {
            (DbError::AuthUnsupported("mysql_clear_password".to_string()), false)
        }
        mysql_async::Error::Driver(mysql_async::DriverError::MysqlOldPasswordDisabled) => {
            (DbError::AuthUnsupported("mysql_old_password".to_string()), false)
        }
        _ => (my_error(e), false),
    }
}

/// ER_SECURE_TRANSPORT_REQUIRED: the server takes encrypted connections only.
const SECURE_TRANSPORT_REQUIRED: u16 = 3159;

/// ER_LOCK_WAIT_TIMEOUT: a statement waited for a lock past the session's limit (a row lock's
/// `innodb_lock_wait_timeout`, a metadata lock's `lock_wait_timeout`). `NOWAIT` fails with an
/// error of its own.
const LOCK_WAIT_TIMEOUT: u16 = 1205;

/// ER_CLIENT_INTERACTION_TIMEOUT: the server closed a session idle past its `wait_timeout`
/// (MySQL 8.0.24 and later say so before they close it).
const CLIENT_INTERACTION_TIMEOUT: u16 = 4031;

/// The server's error `code` as the MySQL client shows it (`ERROR 1146 (42S02): Table 'shop.x'
/// doesn't exist`): a lock wait past the limit and a close for being idle as what they are,
/// anything else as the server's message.
pub(crate) fn server_error(code: u16, state: &str, message: &str) -> DbError {
    let text = format!("ERROR {code} ({state}): {message}");
    match code {
        LOCK_WAIT_TIMEOUT => DbError::LockTimeout(text),
        CLIENT_INTERACTION_TIMEOUT => DbError::IdleSessionTimeout(text),
        _ => DbError::Server(text),
    }
}

/// A client error as a [`DbError`]: the server's own message as the MySQL client shows it
/// (`ERROR 1146 (42S02): Table 'shop.x' doesn't exist`), a closed connection, else a fault
/// (an io error's kind, the whole text as its detail).
pub(crate) fn my_error(e: &mysql_async::Error) -> DbError {
    match e {
        mysql_async::Error::Server(s) if s.code == SECURE_TRANSPORT_REQUIRED => DbError::TlsRequired,
        mysql_async::Error::Server(s) => server_error(s.code, &s.state, &s.message),
        mysql_async::Error::Driver(mysql_async::DriverError::ConnectionClosed) => DbError::Closed,
        // The TLS handshake's failure.
        mysql_async::Error::Io(mysql_async::IoError::Io(io)) if handshake_problem(io).is_some() => {
            DbError::Tls(handshake_problem(io).expect("checked"))
        }
        mysql_async::Error::Io(mysql_async::IoError::Io(io)) => match io.kind() {
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe => DbError::Closed,
            _ => DbError::Connection(io_fault(io)),
        },
        e => DbError::Connection(Fault::new(FaultKind::Other, e.to_string())),
    }
}

/// The problem of a failed TLS handshake an io error carries ([`TlsFailed`]). A TLS error of a
/// connection that is up (a broken record) is the connection's failure, not the handshake's.
fn handshake_problem(io: &std::io::Error) -> Option<TlsProblem> {
    io.get_ref().and_then(|e| e.downcast_ref::<TlsFailed>()).map(|TlsFailed(p)| p.clone())
}

/// How long asking the server whether a stopped statement is still there may take.
const STILL_THERE_WAIT: Duration = Duration::from_secs(5);

/// ER_KILL_DENIED_ERROR: the statement is not the user's to kill.
const KILL_DENIED: u16 = 1095;

/// How often a refused kill is sent, and how long after the one before: the statement may run
/// as another account only for a moment (a `SQL SECURITY DEFINER` function called for every
/// row), and be the user's own again at the next try. Such a statement was seen refusing up to
/// 5 tries in a row, 100 ms apart; one inside a definer procedure refuses every one, and is cut
/// some 0.7 s later than at the first.
const KILL_TRIES: u32 = 8;
const KILL_RETRY: Duration = Duration::from_millis(100);

/// What became of the last `KILL QUERY` sent for the session's statement since the session
/// last cleared it ([`Killer::clear`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kill {
    /// None was answered.
    None,
    /// The server took it (then): the statement stops at its next check. Its rollback may
    /// take long, and say nothing meanwhile.
    Taken(Instant),
    /// It did not take: the statement runs on.
    Failed(KillFailure),
}

/// Why a kill did not take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillFailure {
    /// The server refused it (its error code): MySQL does when the statement runs as another
    /// account (inside a `SQL SECURITY DEFINER` routine, it is its definer's).
    Refused(u16),
    /// It could not be sent: the connection to send it on did not open in time.
    NotSent,
    /// It was sent and got no answer within [`KILL_TIMEOUT`]: whether it took is not known, and
    /// it may still be carried out.
    NotAnswered,
}

/// A kill of the running statement sent that never got its answer within [`KILL_TIMEOUT`]
/// ([`KillFailure::NotAnswered`]).
pub(crate) fn kill_not_answered() -> CancelFailure {
    CancelFailure::NotAnswered(KILL_TIMEOUT)
}

/// A kill sent again that never got its answer within [`KILL_TIMEOUT`] while its statement
/// answered ([`Gate::close`]): it may still be carried out on whatever the connection runs next.
pub(crate) fn kill_pending() -> CancelFailure {
    CancelFailure::StopPending(KILL_TIMEOUT)
}

/// What a kill sent again while a request waits ([`Killer::try_again`]) is sent under: open while
/// the request waits, closed by its owner once the statement answered. Each kill is sent under
/// its lock, only while it is open; closing takes the lock first, so it waits for a kill being
/// sent to be answered. No kill is sent for a statement that answered, and none sent while it
/// ran reaches the session's next statement: MySQL drops a `KILL QUERY` that finds the
/// connection idle, and the next statement is sent only once the gate is closed.
pub(crate) struct Gate {
    open: AtomicBool,
    /// A kill was written and its answer has not come (yet, or ever: given up on). One given up
    /// on stays so: a kill sent after it and answered does not say the first was.
    unanswered: AtomicBool,
    sending: tokio::sync::Mutex<()>,
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Gate {
            open: AtomicBool::new(true),
            unanswered: AtomicBool::new(false),
            sending: tokio::sync::Mutex::new(()),
        })
    }

    fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// The statement answered: no kill is sent for it any more. A kill being sent is waited for
    /// first, as long as one may take ([`KILL_TIMEOUT`]); with none, closing waits for nothing.
    /// `false`: one was sent and never answered (the server stalls, or the kill was given up
    /// on). It may still be carried out at any time, on whatever the session runs then: the
    /// session's connection is not to run anything any more.
    pub(crate) async fn close(&self) -> bool {
        self.quiet(|| self.open.store(false, Ordering::SeqCst)).await
    }

    /// Do `then` once no kill is being sent under the gate (one being sent is waited for as
    /// [`Gate::close`] does); `false`: one was sent and never answered.
    async fn quiet(&self, then: impl FnOnce()) -> bool {
        let sending = tokio::time::timeout(KILL_TIMEOUT, self.sending.lock()).await;
        then();
        sending.is_ok() && !self.unanswered.load(Ordering::SeqCst)
    }
}

/// What a cancel needs: a connection of its own over the session's route, as the session's
/// user, that runs `KILL QUERY` of the session's connection id. It stops the statement the
/// session runs, never the session itself, and never another session. What became of the kill
/// is kept for the session ([`Kill`]), which does not read on after one that did not take.
#[derive(Clone)]
pub(crate) struct Killer {
    route: Route,
    opts: Opts,
    id: u64,
    kill: Arc<watch::Sender<Kill>>,
    /// What the user's kills are sent under ([`Killer::kill_query_if`]), for the session's whole
    /// life: it is never closed, the session waits on it whenever its statement ends
    /// ([`Killer::idle`]).
    sent: Arc<Gate>,
    /// Why the last kill that could not be sent was not (the dial, the login, the server's
    /// connection failing, or no answer in time).
    unsent: Arc<std::sync::Mutex<Option<DbError>>>,
}

impl Killer {
    pub(crate) fn new(route: Route, opts: Opts, id: u64) -> Self {
        Self {
            route,
            opts,
            id,
            kill: Arc::new(watch::Sender::new(Kill::None)),
            sent: Gate::new(),
            unsent: Arc::default(),
        }
    }

    /// The session's statement is no longer on its way nor running: `busy` (what the user's kills
    /// ask before they are sent) is cleared once no kill is being sent (or one took as long as a
    /// kill may), so none is sent for it any more, and none sent for it reaches what the session
    /// runs next. `false`: one was sent and never answered (the statement ended meanwhile, or the
    /// kill was given up on): it may still be carried out at any time, on whatever the session
    /// runs then, and the session's connection is not to run anything any more.
    pub(crate) async fn idle(&self, busy: &AtomicBool) -> bool {
        self.sent.quiet(|| busy.store(false, Ordering::SeqCst)).await
    }

    /// A kill that could not be sent, with why ([`KillFailure::NotSent`]).
    pub(crate) fn not_sent(&self) -> CancelFailure {
        let why = self.unsent.lock().ok().and_then(|w| w.clone());
        CancelFailure::NotSent(Box::new(why.unwrap_or(DbError::NoAnswer(KILL_TIMEOUT))))
    }

    /// Ask the server to stop what the session runs. What became of it is kept ([`Kill`]).
    pub(crate) async fn kill_query(&self) {
        self.kill_query_if(|| true).await;
    }

    /// [`Killer::kill_query`], if `still` holds once the connection to send it on is open (the
    /// login can take a while, through a tunnel most: by then the statement may have ended and
    /// another run begun, which the kill must not stop). What became of it is kept only while
    /// `still` holds: it says nothing about the statement of another run. A refusal is tried
    /// again a few times first ([`KILL_TRIES`]). Each try is asked and sent under the session's
    /// gate ([`Killer::idle`]): `still` is asked again while the session cannot end its statement,
    /// and an answer that never comes is known to the session.
    pub(crate) async fn kill_query_if(&self, still: impl Fn() -> bool) {
        self.send_kill(still, false).await;
    }

    /// [`Killer::kill_query_if`]; `only_taken`: what became of it is kept only if the server
    /// took it.
    async fn send_kill(&self, still: impl Fn() -> bool, only_taken: bool) {
        // The server's last answer, kept past the time limit: a slow route that got a refusal
        // (or the kill taken) before it ran out says so, never that nothing was sent.
        let answered = std::sync::Mutex::new(None);
        // Why it could not be sent, when it could not.
        let unsent = std::sync::Mutex::new(None);
        // A kill was written and its answer has not come: one given up on was sent.
        let written = AtomicBool::new(false);
        let why = |e: DbError| {
            if let Ok(mut w) = unsent.lock() {
                *w = Some(e);
            }
        };
        let kill = async {
            let stream = self.route.dial(DIAL_TIMEOUT).await.map_err(&why).ok()?;
            let (_wire, conn, _) = login(&self.route, stream, self.opts.clone()).await;
            let mut conn = conn.map_err(|e| why(connect_error(&e).0)).ok()?;
            let mut sent = None;
            for n in 0..KILL_TRIES {
                if n > 0 {
                    tokio::time::sleep(KILL_RETRY).await;
                }
                let _sending = self.sent.sending.lock().await;
                if !still() {
                    break;
                }
                // Set until the answer comes: a kill given up on halfway stays so.
                let before = self.sent.unanswered.swap(true, Ordering::SeqCst);
                written.store(true, Ordering::SeqCst);
                let answer = conn.query_drop(format!("KILL QUERY {}", self.id)).await;
                written.store(false, Ordering::SeqCst);
                self.sent.unanswered.store(before, Ordering::SeqCst);
                let kill = match answer {
                    Ok(()) => Kill::Taken(Instant::now()),
                    Err(mysql_async::Error::Server(e)) => Kill::Failed(KillFailure::Refused(e.code)),
                    Err(e) => {
                        why(my_error(&e));
                        Kill::Failed(KillFailure::NotSent)
                    }
                };
                if let Ok(mut a) = answered.lock() {
                    *a = Some(kill);
                }
                sent = Some(kill);
                if kill != Kill::Failed(KillFailure::Refused(KILL_DENIED)) {
                    break;
                }
            }
            quit(conn).await;
            Some(sent)
        };
        let kill = match tokio::time::timeout(KILL_TIMEOUT, kill).await {
            // Not sent: the statement it was for ended.
            Ok(Some(None)) => return,
            Ok(Some(Some(kill))) => kill,
            // A kill written and not answered may still be carried out, whatever an earlier try
            // got: said so, not the earlier answer.
            Err(_) if written.load(Ordering::SeqCst) => Kill::Failed(KillFailure::NotAnswered),
            Ok(None) | Err(_) => answered.lock().ok().and_then(|a| *a).unwrap_or(Kill::Failed(KillFailure::NotSent)),
        };
        if !only_taken || matches!(kill, Kill::Taken(_)) {
            if kill == Kill::Failed(KillFailure::NotSent)
                && let Ok(mut w) = self.unsent.lock()
            {
                *w = Some(unsent.lock().ok().and_then(|w| w.clone()).unwrap_or(DbError::NoAnswer(KILL_TIMEOUT)));
            }
            self.record(kill, still);
        }
    }

    /// Send the kill again, on a task of its own, after the server refused it: kept only if the
    /// server takes it now (a statement that ran as another account only for a moment), never a
    /// worse answer. It is sent only while `gate` is open, under its lock ([`Gate`]).
    pub(crate) fn try_again(&self, gate: &Arc<Gate>) {
        let (killer, gate) = (self.clone(), gate.clone());
        tokio::spawn(async move {
            let again = async {
                let stream = killer.route.dial(DIAL_TIMEOUT).await.ok()?;
                let (_wire, conn, _) = login(&killer.route, stream, killer.opts.clone()).await;
                let mut conn = conn.ok()?;
                {
                    let _sending = gate.sending.lock().await;
                    if gate.is_open() {
                        // Set until the answer comes: a kill given up on halfway stays so.
                        let before = gate.unanswered.swap(true, Ordering::SeqCst);
                        let taken = conn.query_drop(format!("KILL QUERY {}", killer.id)).await.is_ok();
                        gate.unanswered.store(before, Ordering::SeqCst);
                        if taken {
                            killer.record(Kill::Taken(Instant::now()), || gate.is_open());
                        }
                    }
                }
                quit(conn).await;
                Some(())
            };
            let _ = tokio::time::timeout(KILL_TIMEOUT, again).await;
        });
    }

    /// Keep `kill` if `still` holds, asked under the lock [`Killer::clear`] takes too: a run
    /// that begins (and clears) between the two never gets the kill of the one before.
    pub(crate) fn record(&self, kill: Kill, still: impl Fn() -> bool) {
        self.kill.send_if_modified(|k| {
            let keep = still();
            if keep {
                *k = kill;
            }
            keep
        });
    }

    /// Whether the server still has the session's connection running a statement, asked on a
    /// connection of its own (a user sees their own threads): `false` when it does not, or does
    /// not answer within [`STILL_THERE_WAIT`].
    pub(crate) async fn still_there(&self) -> bool {
        let ask = async {
            let stream = self.route.dial(DIAL_TIMEOUT).await.ok()?;
            let (_wire, conn, _) = login(&self.route, stream, self.opts.clone()).await;
            let mut conn = conn.ok()?;
            let sql = format!(
                "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {} AND COMMAND = 'Query'",
                self.id
            );
            let n: Option<u64> = conn.query_first(sql).await.ok()?;
            quit(conn).await;
            Some(n == Some(1))
        };
        matches!(tokio::time::timeout(STILL_THERE_WAIT, ask).await, Ok(Some(true)))
    }

    /// Forget what became of the kills sent so far: a new run, or a kill of the session's own,
    /// begins.
    pub(crate) fn clear(&self) {
        self.kill.send_replace(Kill::None);
    }

    /// What became of the last kill, and of the ones that follow.
    pub(crate) fn watch(&self) -> watch::Receiver<Kill> {
        self.kill.subscribe()
    }

    /// Resolves once a kill did not take, with why.
    pub(crate) async fn refused(&self) -> KillFailure {
        let mut kills = self.watch();
        match kills.wait_for(|k| matches!(k, Kill::Failed(_))).await.map(|k| *k) {
            Ok(Kill::Failed(f)) => f,
            _ => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests;
