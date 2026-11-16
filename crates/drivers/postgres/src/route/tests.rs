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

/// A stream to the server whose bytes from it are held back, once a cancel request's 16 bytes
/// may have been written (a TLS negotiation's 8 before them still gets its answer), until
/// `open`: the server's close after a cancel request does not get through before.
struct Gated {
    inner: tokio::net::TcpStream,
    /// Bytes written to the server so far.
    wrote: Arc<AtomicU64>,
    open: Arc<AtomicBool>,
    waker: Arc<std::sync::Mutex<Option<std::task::Waker>>>,
}

impl tokio::io::AsyncRead for Gated {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if !self.open.load(Ordering::SeqCst) && self.wrote.load(Ordering::SeqCst) >= 16 {
            *self.waker.lock().unwrap() = Some(cx.waker().clone());
            if !self.open.load(Ordering::SeqCst) {
                return std::task::Poll::Pending;
            }
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for Gated {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let wrote = std::pin::Pin::new(&mut self.inner).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &wrote {
            self.wrote.fetch_add(*n as u64, Ordering::SeqCst);
        }
        wrote
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Dials [`Gated`] streams that share one gate.
#[derive(Clone, Default)]
pub(crate) struct GatedDialer {
    pub(crate) wrote: Arc<AtomicU64>,
    open: Arc<AtomicBool>,
    waker: Arc<std::sync::Mutex<Option<std::task::Waker>>>,
}

impl GatedDialer {
    pub(crate) fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        if let Some(w) = self.waker.lock().unwrap().take() {
            w.wake();
        }
    }
}

impl datarig_core::transport::Dialer for GatedDialer {
    fn dial(&self, host: &str, port: u16) -> futures::future::BoxFuture<'static, Result<BoxedStream, DialError>> {
        let (host, gate) = (host.to_string(), self.clone());
        Box::pin(async move {
            let inner = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|e| DialError::Failed(Fault::other(e.to_string())))?;
            let stream = Gated { inner, wrote: gate.wrote, open: gate.open, waker: gate.waker };
            Ok(Box::new(stream) as BoxedStream)
        })
    }
}

/// A user's cancel counts as on its way until the server has closed its connection, not only
/// until it is written: [`Hold::hold`] waits for that, so a cancel the server has not passed
/// on yet cannot land on what the session sends next. The server's close is held back here
/// (the cancel's stream gives nothing from the server until let through).
#[tokio::test]
async fn the_hold_waits_until_the_server_took_a_cancel_in() {
    let url = match std::env::var("DATARIG_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            if std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_PG_URL must be set when DATARIG_REQUIRE_PG=1");
            }
            eprintln!("SKIPPED the_hold_waits_until_the_server_took_a_cancel_in: set DATARIG_TEST_PG_URL");
            return;
        }
    };
    let mut cfg: Config = url.parse().expect("a test database URL");
    cfg.ssl_mode(tokio_postgres::config::SslMode::Disable);
    let (client, conn) = cfg.connect(tokio_postgres::NoTls).await.expect("connect");
    let conn = tokio::spawn(conn);
    let dialer = GatedDialer::default();
    let route = route(&cfg, DialerRef(Arc::new(dialer.clone()))).expect("a route");
    let tls = PgTls::new(datarig_core::tls::TlsSettings::plain());
    let cancel = Cancel::new(client.cancel_token(), Some(route), tls);
    let asking = cancel.clone();
    let asked = tokio::spawn(async move { asking.ask(asking.unsent.request(), || true).await });
    // A cancel request is 16 bytes.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while dialer.wrote.load(Ordering::SeqCst) < 16 {
        assert!(std::time::Instant::now() < deadline, "the cancel was never written");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let early = tokio::time::timeout(Duration::from_millis(300), cancel.hold.hold()).await;
    assert!(early.is_err(), "the hold came while the cancel was still on its way");
    dialer.open();
    tokio::time::timeout(Duration::from_secs(5), cancel.hold.hold()).await.expect("held once the server took it");
    cancel.hold.release();
    asked.await.expect("asked");
    assert!(tokio::time::timeout(Duration::from_millis(100), cancel.unsent.current()).await.is_err(), "it was sent");
    drop(client);
    let _ = conn.await;
}
