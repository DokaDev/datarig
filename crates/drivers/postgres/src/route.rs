//! How a session reaches its server: directly, or through a dialer (a tunnel).
//! A session with a [`Route`] opens its connection with `Config::connect_raw` over a stream the
//! dialer opens, and every cancel request it sends ([`Cancel`]) dials a stream of its own the
//! same way (a cancel request is a connection of its own; a direct one connects to the host the
//! session's config names).

use crate::connect::db_error;
use datarig_core::driver::DbError;
use datarig_core::fault::Fault;
use datarig_core::transport::{BoxedStream, DialError, DialerRef};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::watch;
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

/// How long a cancel request may take to reach the server (its stream through a dialer
/// included).
pub const CANCEL_DIAL: Duration = Duration::from_secs(10);

/// What a session needs to ask the server to cancel its running statement.
#[derive(Clone)]
pub(crate) struct Cancel {
    pub(crate) token: CancelToken,
    pub(crate) route: Option<Route>,
    /// The user's cancels that could not be sent, for the session task to see.
    pub(crate) unsent: Unsent,
}

impl Cancel {
    pub(crate) fn new(token: CancelToken, route: Option<Route>) -> Self {
        Self { token, route, unsent: Unsent::default() }
    }

    /// Send a cancel request (directly, or through the route), within [`CANCEL_DIAL`]; why it
    /// could not be sent.
    pub(crate) async fn send(&self) -> Result<(), DbError> {
        let sent = async {
            match &self.route {
                None => self.token.cancel_query(NoTls).await.map_err(|e| db_error(&e)),
                Some(route) => {
                    let stream = route.dial(CANCEL_DIAL).await?;
                    self.token.cancel_query_raw(stream, NoTls).await.map_err(|e| db_error(&e))
                }
            }
        };
        tokio::time::timeout(CANCEL_DIAL, sent).await.unwrap_or(Err(DbError::NoAnswer(CANCEL_DIAL)))
    }

    /// The user's cancel ([`Cancel::send`]), asked during the session's request `request`
    /// ([`Unsent::request`], taken when the user asked). One that could not be sent is noted for
    /// that request ([`Unsent`]): the session must not wait for a statement it cannot stop.
    pub(crate) async fn ask(&self, request: u64) {
        if let Err(why) = self.send().await {
            self.unsent.note(request, why);
        }
    }
}

/// The user's cancels that could not be sent, shared by the session's canceller and its task.
/// Each is noted with the number of the session's request it was asked during (the task counts
/// its requests, [`Unsent::next_request`]), so one that fails after its request ended is never
/// taken for a later request's.
#[derive(Clone)]
pub(crate) struct Unsent {
    request: Arc<AtomicU64>,
    noted: Arc<watch::Sender<Option<(u64, DbError)>>>,
}

impl Default for Unsent {
    fn default() -> Self {
        Self { request: Arc::default(), noted: Arc::new(watch::channel(None).0) }
    }
}

impl Unsent {
    /// The session begins its next request: cancels asked before belong to an earlier one.
    pub(crate) fn next_request(&self) {
        self.request.fetch_add(1, Ordering::SeqCst);
    }

    /// The number of the session's current request.
    pub(crate) fn request(&self) -> u64 {
        self.request.load(Ordering::SeqCst)
    }

    /// A cancel asked during request `request` could not be sent (`why`).
    fn note(&self, request: u64, why: DbError) {
        self.noted.send_replace(Some((request, why)));
    }

    /// Resolves, with the reason, once a cancel asked during the current request could not be
    /// sent.
    pub(crate) async fn current(&self) -> DbError {
        let mut noted = self.noted.subscribe();
        loop {
            if let Some((request, why)) = &*noted.borrow_and_update()
                && *request == self.request.load(Ordering::SeqCst)
            {
                return why.clone();
            }
            // The sender lives as long as `self`: this does not end.
            if noted.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cancel that failed is seen only by the request it was asked during: one asked before
    /// the session moved on to its next request is never taken for the new request's.
    #[tokio::test]
    async fn a_failed_cancel_belongs_to_the_request_it_was_asked_during() {
        let unsent = Unsent::default();
        let asked = unsent.request();
        unsent.next_request();
        unsent.note(asked, DbError::Closed);
        let late = tokio::time::timeout(Duration::from_millis(100), unsent.current()).await;
        assert!(late.is_err(), "an earlier request's failure was taken for the current one's");
        unsent.note(unsent.request(), DbError::Closed);
        let now = tokio::time::timeout(Duration::from_secs(1), unsent.current()).await;
        assert_eq!(now.ok(), Some(DbError::Closed));
    }
}
