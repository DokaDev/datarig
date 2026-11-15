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
) -> Result<(), Error>
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
    // Neither end of file, an error, nor the bound running out unsends it.
    if let Some(Settle { sent, within }) = settle {
        let _ = sent.send(());
        let mut byte = [0u8; 1];
        let closed = async {
            while let Ok(1..) = stream.read(&mut byte).await {}
        };
        let _ = tokio::time::timeout(within, closed).await;
    }
    // datarig: the request is sent once it is written and flushed; the server answers nothing.
    // The stream is dropped, not shut down: closing the write side may fail or never end (a
    // tunnel's channel the far end already closed, or whose end of file cannot be queued), and
    // neither unsends the request.
    drop(stream);

    Ok(())
}

/// datarig: what a cancel request waits for after it is sent ([`cancel_query_raw`]): `sent` hears
/// that it was written and flushed, then the server is given `within` to close the connection.
pub struct Settle {
    /// Told once the request is written and flushed.
    pub sent: oneshot::Sender<()>,
    /// How long the server is given to close the connection after that.
    pub within: Duration,
}
