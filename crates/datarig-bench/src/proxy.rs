//! A TCP proxy in front of a database server (PostgreSQL, MySQL) that adds a fixed one-way
//! latency to each direction and counts round trips.
//!
//! A *flight* is what the client sends before it hears from the server again: every read from
//! the client that comes after something from the server starts a new one. A request that waits
//! for the answer to the one before costs a flight each; messages pipelined into one write (or
//! written back to back without waiting) share one. With a latency of a few milliseconds the
//! count does not depend on the machine's speed: the server cannot answer before the client's
//! next write of the same flight has reached the proxy.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::unbounded_channel;

#[derive(Default)]
pub struct Counts {
    flights: AtomicU64,
    /// The server spoke since the client's last read: the client's next read is a new flight.
    server_spoke: AtomicBool,
    /// `Sync` and simple `Query` messages from the client: each is answered by one
    /// `ReadyForQuery`.
    requests: AtomicU64,
    /// `flights` when something from the server was last passed on to the client: a result is
    /// in front of the client after this many round trips, whatever the client sends right
    /// after it (a `COMMIT` it does not wait for).
    delivered: AtomicU64,
}

impl Counts {
    pub fn flights(&self) -> u64 {
        self.flights.load(SeqCst)
    }

    pub fn requests(&self) -> u64 {
        self.requests.load(SeqCst)
    }

    pub fn delivered(&self) -> u64 {
        self.delivered.load(SeqCst)
    }
}

pub struct Proxy {
    pub port: u16,
    pub counts: Arc<Counts>,
}

/// Listen on a free local port and relay every connection to `host:port` (a PostgreSQL server:
/// its requests are counted), each direction `one_way` late.
pub async fn start(host: String, port: u16, one_way: Duration) -> std::io::Result<Proxy> {
    start_for(host, port, one_way, true).await
}

/// [`start`] for a server of another protocol (MySQL): flights only, no requests counted.
pub async fn start_raw(host: String, port: u16, one_way: Duration) -> std::io::Result<Proxy> {
    start_for(host, port, one_way, false).await
}

async fn start_for(host: String, port: u16, one_way: Duration, pg: bool) -> std::io::Result<Proxy> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local = listener.local_addr()?.port();
    let counts = Arc::new(Counts { server_spoke: AtomicBool::new(true), ..Counts::default() });
    let c = counts.clone();
    tokio::spawn(async move {
        while let Ok((down, _)) = listener.accept().await {
            let Ok(up) = TcpStream::connect((host.as_str(), port)).await else { return };
            let _ = down.set_nodelay(true);
            let _ = up.set_nodelay(true);
            let (dr, dw) = down.into_split();
            let (ur, uw) = up.into_split();
            tokio::spawn(relay(dr, uw, c.clone(), true, pg, one_way));
            tokio::spawn(relay(ur, dw, c.clone(), false, pg, one_way));
        }
    });
    Ok(Proxy { port: local, counts })
}

/// Relay one direction: each read is passed on `delay` after it arrived, without holding up
/// the reads behind it. The client's side of a PostgreSQL connection (`pg`) is split into
/// protocol messages to count requests (its first message, the startup message, has no type
/// byte).
async fn relay(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    counts: Arc<Counts>,
    client: bool,
    pg: bool,
    delay: Duration,
) {
    let (late_tx, mut late_rx) = unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    let c = counts.clone();
    tokio::spawn(async move {
        while let Some((at, bytes)) = late_rx.recv().await {
            tokio::time::sleep_until(at).await;
            if to.write_all(&bytes).await.is_err() {
                return;
            }
            if !client {
                c.delivered.store(c.flights(), SeqCst);
            }
        }
    });
    let mut buf = Vec::new();
    let mut untyped = true;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = match from.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if client {
            if counts.server_spoke.swap(false, SeqCst) {
                counts.flights.fetch_add(1, SeqCst);
            }
            if pg {
                buf.extend_from_slice(&chunk[..n]);
                loop {
                    let head = usize::from(!untyped);
                    if buf.len() < head + 4 {
                        break;
                    }
                    let len = u32::from_be_bytes([buf[head], buf[head + 1], buf[head + 2], buf[head + 3]]) as usize;
                    if buf.len() < head + len {
                        break;
                    }
                    if !untyped && matches!(buf[0], b'S' | b'Q') {
                        counts.requests.fetch_add(1, SeqCst);
                    }
                    untyped = false;
                    buf.drain(..head + len);
                }
            }
        } else {
            counts.server_spoke.store(true, SeqCst);
        }
        if late_tx.send((tokio::time::Instant::now() + delay, chunk[..n].to_vec())).is_err() {
            return;
        }
    }
}
