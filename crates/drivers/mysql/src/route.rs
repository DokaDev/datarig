//! How a session reaches its server: a TCP connection of its own, or a stream its dialer opens
//! (a channel of an SSH tunnel). Either way the driver holds the stream and hands it to
//! mysql_async, so a connection, a test connection and a cancel's connection ([`Route::dial`])
//! all go the same way, with the profile's TLS ([`RouteTls`]).

use datarig_core::driver::DbError;
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::tls::{TlsMode, TlsSettings};
use datarig_core::transport::{BoxedStream, DialError, DialerRef};
use datarig_tls::Connector;
use std::sync::Arc;
use std::time::Duration;

/// Where the server is, and how to get there.
#[derive(Clone)]
pub(crate) struct Route {
    host: String,
    port: u16,
    /// Through this dialer (as the far end of its transport sees `host`); `None`: directly.
    dialer: Option<DialerRef>,
    /// How its connections are encrypted.
    pub(crate) tls: RouteTls,
}

/// The TLS of a route's connections: the connector (`None`: TLS off), whether a server without
/// TLS is an error, and whether a login may send the password itself over it.
#[derive(Clone, Default)]
pub(crate) struct RouteTls {
    pub(crate) connector: Option<Arc<Connector>>,
    pub(crate) required: bool,
    pub(crate) password_ok: bool,
}

impl Route {
    pub(crate) fn new(host: &str, port: u16, dialer: Option<DialerRef>) -> Self {
        Self { host: host.to_string(), port, dialer, tls: RouteTls::default() }
    }

    /// The route with TLS as `settings` say (files read now; the certificate is checked against
    /// the route's host, the profile's, also through a tunnel). A full `caching_sha2_password`
    /// login may send the password itself over TLS when the server was verified, when the route
    /// does not go through the network on its own (loopback, a tunnel), or when the profile lets
    /// a login trust whoever answers (`key_retrieval`, as it trusts the key such a server
    /// sends): never otherwise, as it would go to whoever is in between.
    pub(crate) fn with_tls(mut self, settings: &TlsSettings, key_retrieval: bool) -> Result<Self, DbError> {
        let connector = Connector::new(settings, &self.host).map_err(DbError::Tls)?;
        self.tls = RouteTls {
            required: settings.mode != TlsMode::Prefer,
            password_ok: settings.mode.verifies() || !self.exposed() || key_retrieval,
            connector: connector.map(Arc::new),
        };
        Ok(self)
    }

    /// The route goes through the network unencrypted: directly (no tunnel) to a server that is
    /// not on this machine.
    pub(crate) fn exposed(&self) -> bool {
        self.dialer.is_none() && !datarig_core::profile::is_loopback(&self.host)
    }

    /// A new stream to the server, within `timeout`.
    pub(crate) async fn dial(&self, timeout: Duration) -> Result<BoxedStream, DbError> {
        match &self.dialer {
            Some(dialer) => match tokio::time::timeout(timeout, dialer.dial(&self.host, self.port)).await {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(e)) => Err(DbError::Transport(e)),
                Err(_) => Err(DbError::Transport(DialError::Timeout(timeout))),
            },
            None => {
                let connect = tokio::net::TcpStream::connect((self.host.as_str(), self.port));
                match tokio::time::timeout(timeout, connect).await {
                    Ok(Ok(s)) => {
                        let _ = s.set_nodelay(true);
                        Ok(Box::new(s) as BoxedStream)
                    }
                    Ok(Err(e)) => Err(DbError::Connection(io_fault(&e))),
                    Err(_) => Err(DbError::Connection(Fault::new(
                        FaultKind::Io(std::io::ErrorKind::TimedOut),
                        format!("no connection to {}:{} within {timeout:?}", self.host, self.port),
                    ))),
                }
            }
        }
    }
}

/// An io error as a fault: a host name that did not resolve, else its kind.
pub(crate) fn io_fault(e: &std::io::Error) -> Fault {
    let kind = if lookup_failed(e) { FaultKind::HostNotFound } else { FaultKind::Io(e.kind()) };
    Fault::new(kind, e.to_string())
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
