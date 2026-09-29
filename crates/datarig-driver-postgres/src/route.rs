//! How a session reaches its server: directly, or through a dialer (a tunnel).
//! A session with a [`Route`] opens its connection with `Config::connect_raw` over a stream the
//! dialer opens, and every cancel request it sends ([`Cancel`]) dials a stream of its own the
//! same way (a cancel request is a connection of its own; a direct one connects to the host the
//! session's config names).

use datarig_core::driver::DbError;
use datarig_core::fault::Fault;
use datarig_core::transport::{BoxedStream, DialError, DialerRef};
use std::time::Duration;
use tokio_postgres::config::Host;
use tokio_postgres::{CancelToken, Config, NoTls};

/// Where a session reaches its server through a dialer: the dialer and the server's host and
/// port (as the far end of the transport sees them).
#[derive(Clone)]
pub(crate) struct Route {
    dialer: DialerRef,
    host: String,
    port: u16,
}

impl Route {
    /// A new stream to the server, within `timeout`.
    pub(crate) async fn dial(&self, timeout: Duration) -> Result<BoxedStream, DbError> {
        match tokio::time::timeout(timeout, self.dialer.dial(&self.host, self.port)).await {
            Ok(Ok(stream)) => Ok(stream),
            Ok(Err(e)) => Err(DbError::Transport(e)),
            Err(_) => Err(DbError::Transport(DialError::Timeout(timeout))),
        }
    }
}

/// The route to the server of `cfg` through `dialer`: its one TCP host (the `hostaddr` when
/// the DSN gives one) and port. A tunnel reaches one host: several, or a Unix socket, cannot
/// be used.
pub(crate) fn route(cfg: &Config, dialer: DialerRef) -> Result<Route, DbError> {
    let unusable = |why: &str| DbError::Settings(Fault::other(format!("a tunnel reaches one TCP host: {why}")));
    let host = match (cfg.get_hosts(), cfg.get_hostaddrs()) {
        (_, [addr]) => addr.to_string(),
        (_, [_, _, ..]) => return Err(unusable("several host addresses")),
        ([Host::Tcp(h)], []) => h.clone(),
        ([], []) => "localhost".to_string(),
        #[cfg(unix)]
        ([Host::Unix(_)], []) => return Err(unusable("a Unix socket")),
        _ => return Err(unusable("several hosts")),
    };
    let port = match cfg.get_ports() {
        [] => 5432,
        [p] => *p,
        _ => return Err(unusable("several ports")),
    };
    Ok(Route { dialer, host, port })
}

/// How long a cancel request waits for its stream through a dialer.
const CANCEL_DIAL: Duration = Duration::from_secs(10);

/// What a session needs to ask the server to cancel its running statement.
#[derive(Clone)]
pub(crate) struct Cancel {
    pub(crate) token: CancelToken,
    pub(crate) route: Option<Route>,
}

impl Cancel {
    /// Send a cancel request (directly, or through the route). A failure is not reported: the
    /// statement then simply runs on.
    pub(crate) async fn send(&self) {
        match &self.route {
            None => {
                let _ = self.token.cancel_query(NoTls).await;
            }
            Some(route) => {
                if let Ok(stream) = route.dial(CANCEL_DIAL).await {
                    let _ = self.token.cancel_query_raw(stream, NoTls).await;
                }
            }
        }
    }
}
