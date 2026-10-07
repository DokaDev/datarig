//! Opening a connection: the profile's server and credentials ([`Target`]), the handshake over
//! a stream of the session's route, the server's version ([`Server`]), and the session's
//! settings in one statement ([`init_sql`]); the session state the server reports
//! ([`Tracked`]); and the cancel's connection of its own ([`Killer`]).

use crate::route::{Route, io_fault};
use crate::wire::Wire;
use datarig_core::driver::{DbError, SessionRole};
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::dsn::{self, Scheme};
use datarig_core::sql::dialect::MySqlMode;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SessionStateChange};
use std::time::Duration;

/// Upper bound for opening a session's connection (the stream, the login, the settings).
pub(crate) const CONNECT_GUARD: Duration = Duration::from_secs(30);

/// How long a connection's stream may take to open (a direct connection's TCP connect, or the
/// dial through a tunnel).
pub(crate) const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a cancel's connection may take to open and send its `KILL QUERY`.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);

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
}

impl Target {
    /// The target of `cfg`, with its server public key file read.
    pub(crate) fn of(cfg: &ConnectionConfig) -> Result<Self, DbError> {
        let settings = |why: String| DbError::Settings(Fault::other(why));
        let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let server_key = cfg.server_public_key_file.as_deref().map(crate::server_key::read).transpose()?;
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
            }),
        }
    }

    /// The client's options for a connection over `route` to `database` (or none) that the
    /// server lists as `program_name` (`performance_schema.session_connect_attrs`). mysql_async
    /// asks the server nothing more after the login (`max_allowed_packet` and `wait_timeout` are
    /// given), never moves to the server's socket file, and never sends the password as it is:
    /// the cleartext method is off, and a full `caching_sha2_password` login over a stream that
    /// is not encrypted (no TLS yet) sends it encrypted with the server's public key. That is
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
///   statement ends, and with it every metadata lock it took);
/// * the metadata session: a lookup waits at most [`META_LOCK_WAIT`] seconds for a metadata
///   lock, a `SELECT` runs at most [`META_TIMEOUT_MS`], and the session is read-only (it only
///   reads the catalog);
/// * a read-only profile's sessions: read-only.
pub(crate) fn init_sql(server: &Server, s: &Settings) -> String {
    let ro = server.read_only_var();
    let mut parts = vec![
        "NAMES utf8mb4".to_string(),
        format!("SESSION session_track_system_variables = 'sql_mode,{ro},sql_select_limit'"),
        "SESSION session_track_schema = ON".to_string(),
        "SESSION sql_mode = @@SESSION.sql_mode".to_string(),
    ];
    if let Some(n) = s.select_limit {
        parts.push(format!("SESSION sql_select_limit = {n}"));
    }
    if s.role == SessionRole::Meta {
        parts.push(format!("SESSION lock_wait_timeout = {META_LOCK_WAIT}"));
        parts.push(server.timeout(META_TIMEOUT_MS));
    }
    if s.read_only || s.role == SessionRole::Meta {
        parts.push(format!("SESSION {ro} = ON"));
    }
    format!("SET {}", parts.join(", "))
}

/// What the server reported of the session (its session state tracking), as far as the driver
/// follows it; `None`: not reported yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tracked {
    pub(crate) sql_mode: Option<String>,
    pub(crate) read_only: Option<bool>,
    pub(crate) select_limit: Option<String>,
    /// The current database (`USE`, `COM_INIT_DB`); `Some("")` for none.
    pub(crate) schema: Option<String>,
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
                            _ => {}
                        }
                    }
                }
                SessionStateChange::Schema(s) => self.schema = Some(s.as_str().into_owned()),
                _ => {}
            }
        }
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
    pub(crate) server: Server,
    pub(crate) tracked: Tracked,
    /// The connection's id on the server (what `KILL QUERY` names).
    pub(crate) id: u64,
}

/// Open a connection to `target` over a stream of `route` with `opts`, check the server's
/// version and set the session up ([`init_sql`]); the session's read-only is checked with the
/// server. A failure says whether it is about the password (`true`: a prompt can help).
pub(crate) async fn open(route: &Route, opts: Opts, settings: Settings) -> Result<Opened, (DbError, bool)> {
    let stream = route.dial(DIAL_TIMEOUT).await.map_err(|e| (e, false))?;
    let (wire, stream) = Wire::new(stream);
    let mut conn = Conn::connect_with_stream(opts, Box::new(stream)).await.map_err(|e| connect_error(&e))?;
    let server = Server { version: conn.server_version(), mariadb: conn.is_mariadb() };
    if let Err(e) = server.check() {
        quit(conn).await;
        return Err((e, false));
    }
    let mut tracked = Tracked::default();
    let set = conn.query_drop(init_sql(&server, &settings)).await;
    set.map_err(|e| (my_error(&e), false))?;
    tracked.update(&conn);
    let read_only = settings.read_only || settings.role == SessionRole::Meta;
    // A server (or a proxy in front of it) that did not report them is asked.
    if tracked.sql_mode.is_none() || (read_only && tracked.read_only.is_none()) {
        let ro = server.read_only_var();
        let ask = format!("SELECT @@SESSION.sql_mode, CAST(@@SESSION.{ro} AS CHAR)");
        let row: Option<(String, String)> = conn.query_first(ask).await.map_err(|e| (my_error(&e), false))?;
        if let Some((mode, ro)) = row {
            tracked.sql_mode = Some(mode);
            tracked.read_only = Some(on(&ro));
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
    Ok(Opened { conn, wire, server, tracked, id })
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

/// A client error as a [`DbError`]: the server's own message as the MySQL client shows it
/// (`ERROR 1146 (42S02): Table 'shop.x' doesn't exist`), a closed connection, else a fault
/// (an io error's kind, the whole text as its detail).
pub(crate) fn my_error(e: &mysql_async::Error) -> DbError {
    match e {
        mysql_async::Error::Server(s) if s.code == SECURE_TRANSPORT_REQUIRED => DbError::TlsRequired,
        mysql_async::Error::Server(s) => DbError::Server(format!("ERROR {} ({}): {}", s.code, s.state, s.message)),
        mysql_async::Error::Driver(mysql_async::DriverError::ConnectionClosed) => DbError::Closed,
        mysql_async::Error::Io(mysql_async::IoError::Io(io)) => match io.kind() {
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe => DbError::Closed,
            _ => DbError::Connection(io_fault(io)),
        },
        e => DbError::Connection(Fault::new(FaultKind::Other, e.to_string())),
    }
}

/// What a cancel needs: a connection of its own over the session's route, as the session's
/// user, that runs `KILL QUERY` of the session's connection id. It stops the statement the
/// session runs, never the session itself, and never another session.
#[derive(Clone)]
pub(crate) struct Killer {
    route: Route,
    opts: Opts,
    id: u64,
}

impl Killer {
    pub(crate) fn new(route: Route, opts: Opts, id: u64) -> Self {
        Self { route, opts, id }
    }

    /// Ask the server to stop what the session runs. A failure is not reported: the statement
    /// then simply runs on.
    pub(crate) async fn kill_query(&self) {
        self.kill_query_if(|| true).await;
    }

    /// [`Killer::kill_query`], if `still` holds once the connection to send it on is open (the
    /// login can take a while, through a tunnel most: by then the statement may have ended and
    /// another run begun, which the kill must not stop).
    pub(crate) async fn kill_query_if(&self, still: impl Fn() -> bool) {
        let kill = async {
            let stream = self.route.dial(DIAL_TIMEOUT).await.ok()?;
            let (_wire, stream) = Wire::new(stream);
            let mut conn = Conn::connect_with_stream(self.opts.clone(), Box::new(stream)).await.ok()?;
            if still() {
                let _ = conn.query_drop(format!("KILL QUERY {}", self.id)).await;
            }
            quit(conn).await;
            Some(())
        };
        let _ = tokio::time::timeout(KILL_TIMEOUT, kill).await;
    }
}

#[cfg(test)]
mod tests;
