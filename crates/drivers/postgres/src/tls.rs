//! TLS for tokio-postgres: datarig's connector ([`datarig_tls::Connector`]) behind tokio-postgres's
//! `MakeTlsConnect`/`TlsConnect`. tokio-postgres sends the SSLRequest itself (the session's
//! `sslmode`: `prefer` goes on unencrypted only when the server answers it has no TLS) and hands
//! the stream over for the handshake; the handshake's outcome is kept for the session to report
//! ([`PgTls::encryption`]). A direct connection checks the certificate against the host it
//! connects to; one through a tunnel, and its cancel requests, against the profile's host
//! ([`PgTls::to`]), never the tunnel's local end.

use datarig_core::fault::Fault;
use datarig_core::tls::{Encryption, TlsMode, TlsProblem, TlsSettings};
use datarig_tls::Connector;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::config::SslMode;
use tokio_postgres::tls::{ChannelBinding, MakeTlsConnect, TlsConnect, TlsStream};

/// A session's TLS settings and what its last handshake got.
#[derive(Clone)]
pub(crate) struct PgTls {
    settings: TlsSettings,
    outcome: Arc<Mutex<Option<Encryption>>>,
}

impl PgTls {
    pub(crate) fn new(settings: TlsSettings) -> Self {
        Self { settings, outcome: Arc::default() }
    }

    /// tokio-postgres's mode for the settings: whether it asks for TLS, and whether a server
    /// without it is an error (what is checked of the certificate is the connector's).
    pub(crate) fn ssl_mode(&self) -> SslMode {
        match self.settings.mode {
            TlsMode::Disable => SslMode::Disable,
            TlsMode::Prefer => SslMode::Prefer,
            TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => SslMode::Require,
        }
    }

    /// The handshake for a server whose certificate names `host`. A new attempt: what an earlier
    /// one got (another host of the same settings) is forgotten.
    pub(crate) fn to(&self, host: &str) -> Result<PgTlsConnect, TlsError> {
        if let Ok(mut o) = self.outcome.lock() {
            *o = None;
        }
        let connector = Connector::new(&self.settings, host).map_err(TlsError)?;
        Ok(PgTlsConnect { connector: connector.map(Arc::new), outcome: self.outcome.clone() })
    }

    /// How the connection whose handshake ran last went: TLS as it was negotiated, else plain.
    pub(crate) fn encryption(&self) -> Encryption {
        self.outcome.lock().ok().and_then(|o| *o).unwrap_or(Encryption::Plain)
    }
}

impl<S> MakeTlsConnect<S> for PgTls
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = PgTlsStream<S>;
    type TlsConnect = PgTlsConnect;
    type Error = TlsError;

    fn make_tls_connect(&mut self, domain: &str) -> Result<PgTlsConnect, TlsError> {
        self.to(domain)
    }
}

/// One handshake, ready.
pub(crate) struct PgTlsConnect {
    connector: Option<Arc<Connector>>,
    outcome: Arc<Mutex<Option<Encryption>>>,
}

impl<S> TlsConnect<S> for PgTlsConnect
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = PgTlsStream<S>;
    type Error = TlsError;
    type Future = Handshake<S>;

    fn connect(self, stream: S) -> Handshake<S> {
        // tokio-postgres asks for a handshake only when its mode is not `disable`, which is when
        // there is a connector.
        match self.connector {
            Some(connector) => {
                Handshake::Running { tls: Box::new(connector.handshake(stream)), connector, outcome: self.outcome }
            }
            None => Handshake::Off,
        }
    }
}

/// A handshake under way: when it ends, what it got is kept for the session.
pub(crate) enum Handshake<S> {
    Running { tls: Box<datarig_tls::Connect<S>>, connector: Arc<Connector>, outcome: Arc<Mutex<Option<Encryption>>> },
    Off,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Future for Handshake<S> {
    type Output = Result<PgTlsStream<S>, TlsError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut *self {
            Handshake::Off => Poll::Ready(Err(TlsError(TlsProblem::Handshake(Fault::other("TLS is off"))))),
            Handshake::Running { tls, connector, outcome } => match Pin::new(&mut **tls).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Err(e)) => Poll::Ready(Err(TlsError(connector.io_problem(&e)))),
                Poll::Ready(Ok(stream)) => {
                    if let Ok(mut o) = outcome.lock() {
                        *o = Some(connector.encryption(&stream));
                    }
                    Poll::Ready(Ok(PgTlsStream(stream)))
                }
            },
        }
    }
}

/// A TLS connection to the server, with SCRAM's channel binding data.
pub(crate) struct PgTlsStream<S>(datarig_tls::TlsStream<S>);

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for PgTlsStream<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for PgTlsStream<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TlsStream for PgTlsStream<S> {
    fn channel_binding(&self) -> ChannelBinding {
        match datarig_tls::server_end_point(&self.0) {
            Some(hash) => ChannelBinding::tls_server_end_point(hash),
            None => ChannelBinding::none(),
        }
    }
}

/// Why the TLS handshake (or getting ready for it) failed, as tokio-postgres carries it.
#[derive(Debug)]
pub(crate) struct TlsError(pub(crate) TlsProblem);

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TLS: {:?}", self.0)
    }
}

impl std::error::Error for TlsError {}
