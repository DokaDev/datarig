use crate::config::{SslMode, SslNegotiation};
use crate::tls::TlsConnect;
use crate::{Error, connect_tls};
use bytes::BytesMut;
use postgres_protocol::message::frontend;
use futures_channel::oneshot;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::time::Duration;

pub async fn cancel_query_raw<S, T>(
    stream: S,
    mode: SslMode,
    negotiation: SslNegotiation,
    tls: T,
    has_hostname: bool,
    process_id: i32,
    secret_key: i32,
    settle: Option<Settle>,
) -> Result<Settled, Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: TlsConnect<S>,
{
    let mut stream = connect_tls::connect_tls(stream, mode, negotiation, tls, has_hostname).await?;

    let mut buf = BytesMut::new();
    frontend::cancel_request(process_id, secret_key, &mut buf);

    stream.write_all(&buf).await.map_err(Error::io)?;
    stream.flush().await.map_err(Error::io)?;
    // datarig: with `settle`, the caller hears that the request is sent, then the server is given
    // `within` to close the connection, which it does once it has passed the request on to the
    // session it cancels: until then the cancel may still land on what the session runs next.
    // Only the end of file says it was passed on; an error or the bound running out leaves it
    // unknown. Neither unsends it.
    let settled = match settle {
        None => Settled::Closed,
        Some(Settle { sent, within }) => {
            let _ = sent.send(());
            let mut byte = [0u8; 1];
            let closed = async {
                loop {
                    match stream.read(&mut byte).await {
                        Ok(0) => return Settled::Closed,
                        Ok(_) => {}
                        Err(_) => return Settled::Unknown,
                    }
                }
            };
            tokio::time::timeout(within, closed).await.unwrap_or(Settled::Unknown)
        }
    };
    // datarig: the request is sent once it is written and flushed; the server answers nothing.
    // The stream is dropped, not shut down: closing the write side may fail or never end (a
    // tunnel's channel the far end already closed, or whose end of file cannot be queued), and
    // neither unsends the request.
    drop(stream);

    Ok(settled)
}

/// datarig: what a cancel request waits for after it is sent ([`cancel_query_raw`]): `sent` hears
/// that it was written and flushed, then the server is given `within` to close the connection.
pub struct Settle {
    /// Told once the request is written and flushed.
    pub sent: oneshot::Sender<()>,
    /// How long the server is given to close the connection after that.
    pub within: Duration,
}

/// datarig: whether the server was seen to take a cancel request in ([`Settle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// It closed the connection: the request was passed on (or none was waited for).
    Closed,
    /// No end of file within the bound, or reading failed: the request may still be on its way.
    Unknown,
}
