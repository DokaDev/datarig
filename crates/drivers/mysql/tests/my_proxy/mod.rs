//! A proxy's part in the tests: an in-process TCP relay in front of the test server that can
//! rewrite the server's greeting (its version text, its connection id; the packet's length
//! follows) and always clears `CLIENT_SSL` in it, so every session through it stays plain. It
//! records the text of every `COM_QUERY` a client sends, in order, and a fault can be set for
//! the next one that starts with a given text ([`Proxy::fault_start`]): the server gets another
//! text, or the connection is closed in its place. It can also leave every new session with a
//! `sql_select_limit` of 0 ([`Proxy::zero_select_limit`]), as a proxy or an `init_connect` that
//! sets it would. Everything else, authentication included, passes through unchanged.

use datarig_core::profile::dsn;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

/// `CLIENT_SSL` in the greeting's lower capability flags.
const CLIENT_SSL: u16 = 0x0800;

/// A `COM_QUERY` packet's first byte.
const COM_QUERY: u8 = 0x03;

/// What happens, once, to the next `COM_QUERY` that starts with a given text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The server gets this text instead.
    Instead(String),
    /// The query never reaches the server: both connections are closed.
    Close,
}

/// The text the relay sends on its own before a session's first statement, in
/// [`Proxy::zero_select_limit`]'s mode.
const ZERO_SELECT_LIMIT: &str = "SET SESSION sql_select_limit = 0";

/// How the server's greeting is rewritten, and what the relay does on its own.
#[derive(Clone, Debug, Default)]
struct Greeting {
    /// The version text sent instead of the server's.
    version: Option<String>,
    /// Added to the connection id (wrapping), so it is not the server's own.
    id_offset: u32,
    /// Whether the session gets [`ZERO_SELECT_LIMIT`] once it logged in.
    zero_select_limit: bool,
}

type Faults = Arc<Mutex<Vec<(String, Fault)>>>;

pub struct Proxy {
    /// `host:port` to connect to instead of the server.
    pub addr: String,
    queries: Arc<Mutex<Vec<String>>>,
    faults: Faults,
    greeting: Arc<Mutex<Greeting>>,
}

impl Proxy {
    /// Listen on a free local port and relay to `upstream` (`host:port`).
    pub async fn start(upstream: &str) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap().to_string();
        let queries: Arc<Mutex<Vec<String>>> = Arc::default();
        let faults: Faults = Arc::default();
        let greeting: Arc<Mutex<Greeting>> = Arc::default();
        let (q, f, g, up) = (queries.clone(), faults.clone(), greeting.clone(), upstream.to_string());
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (q, f, g, up) = (q.clone(), f.clone(), g.clone(), up.clone());
                tokio::spawn(async move {
                    let _ = relay(client, &up, q, f, g).await;
                });
            }
        });
        Proxy { addr, queries, faults, greeting }
    }

    /// The greeting of every connection from now on names version `text`.
    pub fn greet_as(&self, text: &str) {
        self.greeting.lock().unwrap().version = Some(text.to_string());
    }

    /// The greeting of every connection from now on carries a connection id `offset` above the
    /// server's (wrapping), as a proxy that numbers its clients itself.
    pub fn offset_ids(&self, offset: u32) {
        self.greeting.lock().unwrap().id_offset = offset;
    }

    /// Every connection from now on runs [`ZERO_SELECT_LIMIT`] right after its login, before
    /// the client's first statement reaches the server; its answer never reaches the client
    /// (nor the log of [`Proxy::queries`]).
    pub fn zero_select_limit(&self) {
        self.greeting.lock().unwrap().zero_select_limit = true;
    }

    /// The next `COM_QUERY` (on any connection) whose text starts with `start` meets `fault`,
    /// once.
    pub fn fault_start(&self, start: &str, fault: Fault) {
        self.faults.lock().unwrap().push((start.to_string(), fault));
    }

    /// Whether a fault set is still waiting for its query.
    pub fn fault_pending(&self) -> bool {
        !self.faults.lock().unwrap().is_empty()
    }

    /// The text of every `COM_QUERY` the clients sent so far, in order (as sent by the client,
    /// before any fault).
    pub fn queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }

    /// `url` with the server's `host:port` replaced by the proxy's.
    pub fn url(&self, url: &str) -> String {
        let at = url.rfind('@').map_or(url.find("//").unwrap() + 2, |i| i + 1);
        let end = url[at..].find(['/', '?']).map_or(url.len(), |i| at + i);
        format!("{}{}{}", &url[..at], self.addr, &url[end..])
    }
}

/// The server's `host:port` in `url`, read by the profile's URL parser: a URL without a port
/// names the scheme's default one.
pub fn upstream(url: &str) -> String {
    let d = dsn::parse(url).expect("a test server URL");
    let port = d.port.unwrap_or(d.scheme.default_port());
    if d.host.contains(':') { format!("[{}]:{port}", d.host) } else { format!("{}:{port}", d.host) }
}

/// One packet: its sequence number and payload (a payload of `0xFFFFFF` bytes is followed by
/// more of the same message; each part is a packet of its own here).
async fn read_packet(r: &mut OwnedReadHalf) -> std::io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head).await?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], 0]) as usize;
    let mut payload = vec![0; len];
    r.read_exact(&mut payload).await?;
    Ok((head[3], payload))
}

async fn write_packet(w: &mut OwnedWriteHalf, seq: u8, payload: &[u8]) -> std::io::Result<()> {
    let len = (payload.len() as u32).to_le_bytes();
    w.write_all(&[len[0], len[1], len[2], seq]).await?;
    w.write_all(payload).await
}

/// The greeting `payload` (protocol 10) as `how` rewrites it, without `CLIENT_SSL`.
fn rewrite(payload: &[u8], how: &Greeting) -> Vec<u8> {
    let Some(nul) = payload.iter().skip(1).position(|b| *b == 0).map(|p| p + 1) else {
        return payload.to_vec();
    };
    let version = how.version.as_deref().map_or(&payload[1..nul], str::as_bytes);
    let mut out = vec![payload[0]];
    out.extend_from_slice(version);
    out.extend_from_slice(&payload[nul..]);
    // The connection id right after the version's NUL, then 8 bytes of the scramble and a filler,
    // then the lower capability flags.
    let at = 1 + version.len() + 1;
    if let Some(id) = out.get(at..at + 4) {
        let id = u32::from_le_bytes(id.try_into().unwrap()).wrapping_add(how.id_offset);
        out[at..at + 4].copy_from_slice(&id.to_le_bytes());
    }
    let caps = at + 4 + 8 + 1;
    if let Some(c) = out.get(caps..caps + 2) {
        let c = u16::from_le_bytes(c.try_into().unwrap()) & !CLIENT_SSL;
        out[caps..caps + 2].copy_from_slice(&c.to_le_bytes());
    }
    out
}

async fn relay(
    client: TcpStream,
    upstream: &str,
    queries: Arc<Mutex<Vec<String>>>,
    faults: Faults,
    greeting: Arc<Mutex<Greeting>>,
) -> std::io::Result<()> {
    let server = TcpStream::connect(upstream).await?;
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    let (seq, payload) = read_packet(&mut sr).await?;
    let how = greeting.lock().unwrap().clone();
    write_packet(&mut cw, seq, &rewrite(&payload, &how)).await?;
    // The server's next packet, when the relay waits for one of its own statement's answers:
    // taken here and never sent to the client.
    let swallow: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::default();
    let swallowing = swallow.clone();
    tokio::spawn(async move {
        while let Ok((seq, payload)) = read_packet(&mut sr).await {
            if let Some(taken) = swallowing.lock().unwrap().take() {
                let _ = taken.send(());
                continue;
            }
            if write_packet(&mut cw, seq, &payload).await.is_err() {
                break;
            }
        }
        let _ = cw.shutdown().await;
    });
    // A packet that continues the one before (that one was `0xFFFFFF` bytes) is no command.
    let mut continued = false;
    let mut zero_select_limit = how.zero_select_limit;
    while let Ok((seq, payload)) = read_packet(&mut cr).await {
        let mut payload = payload;
        // The client's first command comes once the login is done: the relay's own statement
        // goes first, and its answer (an OK packet) is taken before the client's is sent.
        if !continued && seq == 0 && zero_select_limit {
            zero_select_limit = false;
            let (taken, answered) = oneshot::channel();
            *swallow.lock().unwrap() = Some(taken);
            write_packet(&mut sw, 0, &[&[COM_QUERY], ZERO_SELECT_LIMIT.as_bytes()].concat()).await?;
            if answered.await.is_err() {
                return Ok(());
            }
        }
        if !continued && seq == 0 && payload.first() == Some(&COM_QUERY) {
            let text = String::from_utf8_lossy(&payload[1..]).to_string();
            queries.lock().unwrap().push(text.clone());
            let fault = {
                let mut faults = faults.lock().unwrap();
                let met = faults.iter().position(|(start, _)| text.starts_with(start.as_str()));
                met.map(|at| faults.remove(at).1)
            };
            match fault {
                Some(Fault::Close) => {
                    let _ = sw.shutdown().await;
                    return Ok(());
                }
                Some(Fault::Instead(other)) => payload = [&[COM_QUERY], other.as_bytes()].concat(),
                None => {}
            }
        }
        continued = payload.len() == 0xFF_FFFF;
        write_packet(&mut sw, seq, &payload).await?;
    }
    let _ = sw.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::upstream;

    #[test]
    fn the_upstream_of_a_url_without_a_port_is_on_the_default_port() {
        assert_eq!(upstream("mysql://u:p@127.0.0.1/shop"), "127.0.0.1:3306");
        assert_eq!(upstream("mariadb://u@db.example/shop?ssl-mode=disabled"), "db.example:3306");
        assert_eq!(upstream("mysql://u:p@127.0.0.1:53306/shop"), "127.0.0.1:53306");
        assert_eq!(upstream("mysql://u:p@[::1]/shop"), "[::1]:3306");
        assert_eq!(upstream("mysql://u:p@[::1]:53306/"), "[::1]:53306");
    }
}
