//! The byte stream of a session's connection, shared between mysql_async (which reads and
//! writes it, through [`WireStream`]) and the session ([`Wire`]), which can tell, while it waits
//! for the UI's next command, that the server closed the connection ([`Wire::closed`]):
//! mysql_async has no task of its own that would notice (the server only speaks when asked,
//! except right before it closes a connection, as on `wait_timeout`), so the session watches the
//! stream itself, as the PostgreSQL driver's connection task does.

use datarig_core::transport::BoxedStream;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

struct Shared {
    stream: BoxedStream,
    /// Bytes read while the session waited ([`Wire::closed`]), not yet handed to mysql_async.
    pushback: Vec<u8>,
}

/// The session's handle on its connection's stream.
#[derive(Clone)]
pub(crate) struct Wire(Arc<Mutex<Shared>>);

/// What mysql_async reads and writes (the stream [`Wire::new`] gives the connection).
pub(crate) struct WireStream(Wire);

impl Wire {
    /// A wire over `stream`, and the stream to give the connection.
    pub(crate) fn new(stream: BoxedStream) -> (Wire, WireStream) {
        let wire = Wire(Arc::new(Mutex::new(Shared { stream, pushback: Vec::new() })));
        (wire.clone(), WireStream(wire))
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, Shared>> {
        self.0.lock().map_err(|_| io::Error::other("the connection's stream is unusable"))
    }

    /// Resolves when the server closed the connection, or reading from it failed, while nothing
    /// was asked: the session awaits it only while it waits for a command. What it reads
    /// meanwhile (a server's last words before it closes) is kept for the connection.
    pub(crate) async fn closed(&self) {
        std::future::poll_fn(|cx| self.poll_closed(cx)).await
    }

    fn poll_closed(&self, cx: &mut Context<'_>) -> Poll<()> {
        let Ok(mut s) = self.lock() else { return Poll::Ready(()) };
        let mut chunk = [0u8; 512];
        loop {
            let mut buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut s.stream).poll_read(cx, &mut buf) {
                Poll::Ready(Ok(())) if buf.filled().is_empty() => return Poll::Ready(()),
                Poll::Ready(Ok(())) => {
                    let read = buf.filled().to_vec();
                    s.pushback.extend_from_slice(&read);
                }
                Poll::Ready(Err(_)) => return Poll::Ready(()),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncRead for WireStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let mut s = self.0.lock()?;
        if !s.pushback.is_empty() {
            let n = s.pushback.len().min(buf.remaining());
            buf.put_slice(&s.pushback[..n]);
            s.pushback.drain(..n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut s.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for WireStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut s = self.0.lock()?;
        Pin::new(&mut s.stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut s = self.0.lock()?;
        Pin::new(&mut s.stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut s = self.0.lock()?;
        Pin::new(&mut s.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn bytes_read_while_waiting_reach_the_connection_and_the_end_is_seen() {
        let (client, mut server) = tokio::io::duplex(64);
        let (wire, mut stream) = Wire::new(Box::new(client));
        server.write_all(b"bye").await.unwrap();
        drop(server);
        tokio::time::timeout(std::time::Duration::from_secs(5), wire.closed()).await.unwrap();
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"bye");
    }

    #[tokio::test]
    async fn waiting_does_not_end_while_the_server_is_silent() {
        let (client, _server) = tokio::io::duplex(64);
        let (wire, _stream) = Wire::new(Box::new(client));
        let waited = tokio::time::timeout(std::time::Duration::from_millis(50), wire.closed()).await;
        assert!(waited.is_err(), "nothing was closed");
    }
}
