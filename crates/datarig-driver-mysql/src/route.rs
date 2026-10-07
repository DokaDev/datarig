//! How a session reaches its server: a TCP connection of its own, or a stream its dialer opens
//! (a channel of an SSH tunnel). Either way the driver holds the stream and hands it to
//! mysql_async, so a connection, a test connection and a cancel's connection ([`Route::dial`])
//! all go the same way.

use datarig_core::driver::DbError;
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::transport::{BoxedStream, DialError, DialerRef};
use std::time::Duration;

/// Where the server is, and how to get there.
#[derive(Clone)]
pub(crate) struct Route {
    host: String,
    port: u16,
    /// Through this dialer (as the far end of its transport sees `host`); `None`: directly.
    dialer: Option<DialerRef>,
}

impl Route {
    pub(crate) fn new(host: &str, port: u16, dialer: Option<DialerRef>) -> Self {
        Self { host: host.to_string(), port, dialer }
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
