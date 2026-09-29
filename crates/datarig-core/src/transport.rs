//! How a driver reaches its server when the profile does not connect directly:
//! a [`Dialer`] hands out one byte stream per connection to `host:port` as seen from the far
//! end of the transport (for an SSH tunnel: from its last hop). The driver speaks its protocol
//! over that stream exactly as over a socket of its own; TLS, when the profile asks for it,
//! goes on top of it. A session, its cancel requests and a test connection each dial.
//!
//! A dialer never opens (or reopens) its transport: the app does that before handing it out,
//! with whatever the user has to answer (a host key, a passphrase). A dial on a transport that
//! is not open fails at once ([`DialError::NotOpen`]).
//!
//! Core has no transport of its own; `datarig-ssh` provides the SSH tunnel. The tests of the
//! other crates dial through [`TcpDialer`] (feature `test-util`), a plain TCP connect that
//! counts its dials.

use crate::fault::Fault;
use futures::future::BoxFuture;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// A connection's byte stream.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Stream for T {}

/// A stream a [`Dialer`] handed out.
pub type BoxedStream = Box<dyn Stream>;

/// Opens streams to `host:port` through a transport (see the module documentation).
pub trait Dialer: Send + Sync {
    /// A new stream to `host:port` as the far end of the transport sees them (a name is
    /// resolved there). Dropping the future gives up on the dial.
    fn dial(&self, host: &str, port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>>;
}

/// A dialer as a connect option: compared and printed by identity, never by what it holds.
#[derive(Clone)]
pub struct DialerRef(pub Arc<dyn Dialer>);

impl DialerRef {
    pub fn dial(&self, host: &str, port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        self.0.dial(host, port)
    }
}

impl fmt::Debug for DialerRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Dialer@{:p}", Arc::as_ptr(&self.0).cast::<()>())
    }
}

impl PartialEq for DialerRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DialerRef {}

/// Why the far end of a transport did not open a stream (for SSH: the reason code of a refused
/// channel, RFC 4254 section 5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It is not allowed to connect there (`administratively prohibited`: forwarding is off or
    /// limited to other destinations).
    Prohibited,
    /// It tried and could not connect (`connect failed`: nothing listens there, a firewall, a
    /// name it could not resolve).
    Unreachable,
    /// Any other reason.
    Other,
}

/// Why a dial failed. What the far end said (its message) is in the detail of `Refused`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialError {
    /// The transport is not open (it was closed or lost): reconnect the profile.
    NotOpen,
    /// The far end would not open a stream to `host:port`.
    Refused { host: String, port: u16, reason: Refusal, detail: String },
    /// Reaching the far end failed (an io kind when the cause is one).
    Failed(Fault),
    /// No stream within this time.
    Timeout(Duration),
}

impl DialError {
    /// The raw text (the far end's message or a fault's detail), for the error log only.
    pub fn raw(&self) -> &str {
        match self {
            DialError::Refused { detail, .. } => detail,
            DialError::Failed(f) => &f.detail,
            DialError::NotOpen | DialError::Timeout(_) => "",
        }
    }
}

/// A plain TCP connect as a [`Dialer`], for the tests of other crates: the whole driver runs
/// through the dialer path with nothing in between. Counts its dials.
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct TcpDialer {
    pub dials: std::sync::atomic::AtomicUsize,
}

#[cfg(any(test, feature = "test-util"))]
impl Dialer for TcpDialer {
    fn dial(&self, host: &str, port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        self.dials.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let host = host.to_string();
        Box::pin(async move {
            match tokio::net::TcpStream::connect((host.as_str(), port)).await {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    Ok(Box::new(s) as BoxedStream)
                }
                Err(e) => Err(DialError::Failed(Fault::io(&e))),
            }
        })
    }
}

#[cfg(test)]
mod tests;
