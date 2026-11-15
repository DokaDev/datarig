//! How a session reaches its server: directly, or through a dialer (a tunnel).
//! A session with a [`Route`] opens its connection with `Config::connect_raw` over a stream the
//! dialer opens, and every cancel request it sends ([`Cancel`]) dials a stream of its own the
//! same way (a cancel request is a connection of its own; a direct one connects to the host the
//! session's config names).

use crate::connect::db_error;
use crate::tls::PgTls;
use datarig_core::driver::DbError;
use datarig_core::fault::Fault;
use datarig_core::transport::{BoxedStream, DialError, DialerRef};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::watch;
use tokio_postgres::config::Host;
use tokio_postgres::{CancelToken, Config, Settled};

/// Where a session reaches its server through a dialer: the dialer and the server's host and
/// port (as the far end of the transport sees them).
#[derive(Clone)]
pub(crate) struct Route {
    dialer: DialerRef,
    host: String,
    port: u16,
    /// The name the server's certificate is checked against: the profile's host (the
    /// `hostaddr` only when there is no host name).
    name: String,
}

impl Route {
    /// The name the server's certificate is checked against.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

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
    let name = match cfg.get_hosts() {
        [Host::Tcp(h)] => h.clone(),
        _ => host.clone(),
    };
    Ok(Route { dialer, host, port, name })
}

/// How long a cancel request may take to reach the server (its stream through a dialer
/// included).
pub const CANCEL_DIAL: Duration = Duration::from_secs(10);

/// How long the server is given to close a user's cancel's connection once the request is sent
/// (it does so once it has passed the cancel on to the session): until then [`Hold::hold`]
/// waits, so a cancel on its way does not land on what the session runs next. One still not
/// taken in by then (or whose connection failed) may land on anything the session sends later:
/// the session sends nothing more (a request on its way is still answered) and closes
/// ([`Unsent::pending`]).
pub(crate) const CANCEL_SETTLE: Duration = Duration::from_secs(10);

/// How long a request already on its way is still waited for once a user's cancel stayed
/// unconfirmed ([`CANCEL_SETTLE`]): its answer tells what it did (a `COMMIT` above all), but one
/// that never comes must not hold the session for good. Then the connection is closed and the
/// request's outcome is not known. Of the order of the app's own limit for a cancel to end a run.
pub(crate) const CANCEL_ANSWER_WAIT: Duration = Duration::from_secs(60);

/// What a session needs to ask the server to cancel its running statement.
#[derive(Clone)]
pub(crate) struct Cancel {
    pub(crate) token: CancelToken,
    pub(crate) route: Option<Route>,
    /// The session's TLS: a cancel request is a connection of its own, made as the session's.
    pub(crate) tls: PgTls,
    /// The user's cancels that could not be sent, for the session task to see.
    pub(crate) unsent: Unsent,
    /// While the session runs what a cancel must not land on, the user's cancels wait.
    pub(crate) hold: Arc<Hold>,
    /// How long the server is given to take a user's cancel in ([`CANCEL_SETTLE`]).
    pub(crate) settle: Duration,
    /// How long a request on its way is waited for after that ([`CANCEL_ANSWER_WAIT`]).
    pub(crate) answer_wait: Duration,
}

/// Holds the user's cancels back while the session runs statements a cancel must not land on:
/// inside the user's block an Apply's start, a `Resume`'s, a count's, a count of a column's
/// values', an export's and an `EXPLAIN ANALYZE`'s set their savepoint, and a cancel landing
/// before the savepoint exists would abort the block, the user's earlier work with it. A cancel
/// asked meanwhile is not sent; the run sees it asked and stops itself before its next step.
#[derive(Default)]
pub(crate) struct Hold {
    held: AtomicBool,
    /// Taken by a cancel on its way to the server, until the server took it in
    /// ([`CANCEL_SETTLE`]), so [`Hold::hold`] returns only once none is.
    sending: tokio::sync::RwLock<()>,
}

impl Hold {
    /// Hold the cancels from now on; returns once no cancel is on its way.
    pub(crate) async fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
        drop(self.sending.write().await);
    }

    /// Let the cancels through again.
    pub(crate) fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
    }

    pub(crate) fn held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

/// The user's cancels held back ([`Hold`]) while it lives, unless released for a while or
/// ended.
pub(crate) struct Held<'a> {
    hold: &'a Hold,
    /// Held at all: not once ended, nor when made off ([`Held::when`]).
    on: bool,
}

impl<'a> Held<'a> {
    /// Held from now on; returns once no cancel is on its way.
    pub(crate) async fn new(hold: &'a Hold) -> Held<'a> {
        Held::when(hold, true).await
    }

    /// Held only when `on`; otherwise it never holds anything.
    pub(crate) async fn when(hold: &'a Hold, on: bool) -> Held<'a> {
        if on {
            hold.hold().await;
        }
        Held { hold, on }
    }

    /// Whether it holds at all (released for a while counts).
    pub(crate) fn on(&self) -> bool {
        self.on
    }

    /// Held again after [`Held::release`]; returns once no cancel is on its way.
    pub(crate) async fn hold(&mut self) {
        if self.on {
            self.hold.hold().await;
        }
    }

    /// Let the cancels through for a while.
    pub(crate) fn release(&mut self) {
        if self.on {
            self.hold.release();
        }
    }

    /// No longer held, for good.
    pub(crate) fn end(&mut self) {
        self.release();
        self.on = false;
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

impl Cancel {
    pub(crate) fn new(token: CancelToken, route: Option<Route>, tls: PgTls) -> Self {
        Self {
            token,
            route,
            tls,
            unsent: Unsent::default(),
            hold: Arc::default(),
            settle: CANCEL_SETTLE,
            answer_wait: CANCEL_ANSWER_WAIT,
        }
    }

    /// Send a cancel request (directly, or through the route), within [`CANCEL_DIAL`]; why it
    /// could not be sent.
    pub(crate) async fn send(&self) -> Result<(), DbError> {
        self.send_if(|| true, false).await.map(|_| ())
    }

    /// [`Cancel::send`], unless `still` no longer holds once its stream is open. A cancel
    /// request names no statement, and a tunnel may take its time: by then another run may be
    /// at the server. `settle`: once sent, it returns only when the server closed the cancel's
    /// connection, or after [`Cancel::settle`]: until then the server may not have passed it on,
    /// and it may still land on what the session sends next. Whether the server was seen to take
    /// it in (always, without `settle`).
    async fn send_if(&self, still: impl Fn() -> bool, settle: bool) -> Result<Settled, DbError> {
        let (told, sent) = futures::channel::oneshot::channel();
        let settle = settle.then_some(tokio_postgres::Settle { sent: told, within: self.settle });
        let sending = async {
            let sent = match (&self.route, settle) {
                (None, _) if !still() => return Ok(Settled::Closed),
                (None, None) => self.token.cancel_query(self.tls.clone()).await.map(|()| Settled::Closed),
                (None, Some(settle)) => self.token.cancel_query_settled(self.tls.clone(), settle).await,
                (Some(route), settle) => {
                    let handshake = self.tls.to(route.name()).map_err(|e| DbError::Tls(e.0))?;
                    let stream = route.dial(CANCEL_DIAL).await?;
                    if !still() {
                        return Ok(Settled::Closed);
                    }
                    match settle {
                        None => self.token.cancel_query_raw(stream, handshake).await.map(|()| Settled::Closed),
                        Some(settle) => self.token.cancel_query_raw_settled(stream, handshake, settle).await,
                    }
                }
            };
            sent.map_err(|e| db_error(&e))
        };
        tokio::pin!(sending);
        // Within the bound until it is sent (or it failed); then, while the server takes it in,
        // within the settle bound of its own.
        let first = async {
            tokio::select! {
                biased;
                done = &mut sending => Some(done),
                _ = sent => None,
            }
        };
        match tokio::time::timeout(CANCEL_DIAL, first).await {
            Err(_) => Err(DbError::NoAnswer(CANCEL_DIAL)),
            Ok(Some(done)) => done,
            Ok(None) => sending.await,
        }
    }

    /// The user's cancel ([`Cancel::send`]), asked during the session's request `request`
    /// ([`Unsent::request`], taken when the user asked). One that could not be sent is noted for
    /// that request ([`Unsent`]): the session must not wait for a statement it cannot stop. One
    /// the server was not seen to take in ([`Cancel::settle`]) is noted for the session
    /// ([`Unsent::pending`]): it may land on whatever the session sends next.
    /// `still`: whether the run it is for is still the one at the server.
    /// While cancels are held ([`Hold`]) it is not sent.
    pub(crate) async fn ask(&self, request: u64, still: impl Fn() -> bool) {
        let _sending = self.hold.sending.read().await;
        if self.hold.held() {
            return;
        }
        let hold = self.hold.clone();
        match self.send_if(|| still() && !hold.held(), true).await {
            Ok(Settled::Closed) => {}
            // It may still land on anything: the session sends nothing more.
            Ok(Settled::Unknown) => {
                self.unsent.refuse.store(true, Ordering::SeqCst);
                self.unsent.pending.send_replace(Some(Pending { settle: self.settle, wait: self.answer_wait }));
            }
            Err(why) => self.unsent.note(request, why),
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
    /// A user's cancel was sent and the server was not seen to take it in ([`Pending`]): it may
    /// still land on anything the session sends, whatever request it was asked during.
    pub(crate) pending: Arc<watch::Sender<Option<Pending>>>,
    /// Set with [`Unsent::pending`]: the session's client hands no request to its connection
    /// any more (`Client::refuse_sends_when`), a type lookup in the middle of a preparation
    /// included.
    pub(crate) refuse: Arc<AtomicBool>,
}

/// A user's cancel the server was not seen to take in ([`Unsent::pending`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pending {
    /// How long the server was given to take it in.
    pub(crate) settle: Duration,
    /// How long a request on its way is still waited for.
    pub(crate) wait: Duration,
}

impl Default for Unsent {
    fn default() -> Self {
        Self {
            request: Arc::default(),
            noted: Arc::new(watch::channel(None).0),
            pending: Arc::new(watch::channel(None).0),
            refuse: Arc::default(),
        }
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
pub(crate) mod tests {
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
        swallow: Arc<AtomicBool>,
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
            // A cancel request (16 bytes in one write) never reaches the server.
            if self.swallow.load(Ordering::SeqCst) && buf.len() >= 16 {
                self.wrote.fetch_add(buf.len() as u64, Ordering::SeqCst);
                return std::task::Poll::Ready(Ok(buf.len()));
            }
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
        /// Cancel requests are taken as written and never reach the server.
        pub(crate) swallow: Arc<AtomicBool>,
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
                let stream =
                    Gated { inner, wrote: gate.wrote, open: gate.open, waker: gate.waker, swallow: gate.swallow };
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
        assert!(
            tokio::time::timeout(Duration::from_millis(100), cancel.unsent.current()).await.is_err(),
            "it was sent"
        );
        drop(client);
        let _ = conn.await;
    }
}
