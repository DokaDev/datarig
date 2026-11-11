//! A connection pooler's part in the tests: an in-process TCP proxy in front of
//! the test server that drops the `options` parameter from every startup message, as a pooler
//! does (PgBouncer with `ignore_startup_parameters = options`), and records what each client
//! sends: the text of every simple `Query` and every `Parse`, in the order they go out, and
//! apart from them the row count of every `Execute`. It
//! answers `SSLRequest`/`GSSENCRequest` with "no" and passes everything else through
//! unchanged (authentication included); a cancel request goes to the server as it is. A
//! fault can be set for the next simple query of a given text, or starting with one
//! ([`Proxy::fault`], [`Proxy::fault_start`]): it goes to the server as another statement, or the
//! connection is closed in its place.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// A message a client sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// A simple query (`Q`): its text, maybe several statements.
    Query(String),
    /// A `Parse` (`P`): its statement's text.
    Parse(String),
}

/// What happens, once, to the next simple query of a given text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The server gets this text instead.
    Instead(String),
    /// The start the fault was set for ([`Proxy::fault_start`]) is replaced by this text; what
    /// follows it goes to the server as it is.
    Head(String),
    /// The query reaches the server this much later.
    Delay(std::time::Duration),
    /// The query never reaches the server: both connections are closed.
    Close,
}

/// Which simple query a fault waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Wanted {
    Exactly(String),
    Starting(String),
}

impl Wanted {
    fn is(&self, query: &str) -> bool {
        match self {
            Wanted::Exactly(text) => query == text,
            Wanted::Starting(text) => query.starts_with(text.as_str()),
        }
    }
}

type Faults = Arc<Mutex<Vec<(Wanted, Fault)>>>;

type Rewords = Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>;

pub struct Proxy {
    /// `host:port` to connect to instead of the server.
    pub addr: String,
    /// What the clients sent, in order (all of them together).
    pub sent: Arc<Mutex<Vec<Sent>>>,
    /// How many startup messages had `options` (dropped).
    pub dropped: Arc<Mutex<usize>>,
    /// The rows each `Execute` asked for (0: all), in order.
    pub executes: Arc<Mutex<Vec<i32>>>,
    /// The fault set for the next simple query of a text ([`Proxy::fault`]).
    faults: Faults,
    /// Whether the server's answers are held back ([`Proxy::hold_answers`]).
    held: Arc<watch::Sender<bool>>,
    /// What the server's answers say otherwise ([`Proxy::reword`]).
    rewords: Rewords,
}

impl Proxy {
    /// Listen on a free local port and relay to `upstream` (`host:port`).
    pub async fn start(upstream: &str) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap().to_string();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(Mutex::new(0));
        let executes = Arc::new(Mutex::new(Vec::new()));
        let faults: Faults = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new(watch::channel(false).0);
        let (s, d, e, f, up) = (sent.clone(), dropped.clone(), executes.clone(), faults.clone(), upstream.to_string());
        let h = held.clone();
        let rewords: Rewords = Arc::default();
        let w = rewords.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (s, d, e, f, up, h) = (s.clone(), d.clone(), e.clone(), f.clone(), up.clone(), h.clone());
                let w = w.clone();
                tokio::spawn(async move {
                    let _ = relay(client, &up, s, d, e, f, h, w).await;
                });
            }
        });
        Proxy { addr, sent, dropped, executes, faults, held, rewords }
    }

    /// Hold what the server answers back from the clients (`true`) until told otherwise
    /// (`false`), as an answer slow to arrive.
    pub fn hold_answers(&self, held: bool) {
        self.held.send_replace(held);
    }

    /// What the server answers says `to` wherever it says `from`, as a server that answers in
    /// another language. Of the same length, so no message changes size.
    pub fn reword(&self, from: &str, to: &str) {
        assert_eq!(from.len(), to.len());
        self.rewords.lock().unwrap().push((from.as_bytes().to_vec(), to.as_bytes().to_vec()));
    }

    /// The next simple query whose text is `query` (on any connection) meets `fault`, once;
    /// after the faults set before it for the same text.
    pub fn fault(&self, query: &str, fault: Fault) {
        self.faults.lock().unwrap().push((Wanted::Exactly(query.to_string()), fault));
    }

    /// The next simple query that starts with `start` (on any connection) meets `fault`, once.
    pub fn fault_start(&self, start: &str, fault: Fault) {
        self.faults.lock().unwrap().push((Wanted::Starting(start.to_string()), fault));
    }

    /// Whether a fault set is still waiting for its query.
    pub fn fault_pending(&self) -> bool {
        !self.faults.lock().unwrap().is_empty()
    }

    /// What the clients sent so far.
    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    /// The rows each `Execute` the clients sent so far asked for.
    pub fn executes(&self) -> Vec<i32> {
        self.executes.lock().unwrap().clone()
    }

    /// `url` with the server's `host:port` replaced by the proxy's.
    pub fn url(&self, url: &str) -> String {
        let at = url.rfind('@').map_or(url.find("//").unwrap() + 2, |i| i + 1);
        let end = url[at..].find('/').map_or(url.len(), |i| at + i);
        format!("{}{}{}", &url[..at], self.addr, &url[end..])
    }
}

/// The server's `host:port` in `url`.
pub fn upstream(url: &str) -> String {
    let at = url.rfind('@').map_or(url.find("//").unwrap() + 2, |i| i + 1);
    let end = url[at..].find(['/', '?']).map_or(url.len(), |i| at + i);
    url[at..end].to_string()
}

async fn read_startup(c: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let len = c.read_i32().await? as usize;
    let mut body = vec![0; len - 4];
    c.read_exact(&mut body).await?;
    Ok(body)
}

/// How many bytes at the start of what a server sent are whole messages (a tag, a length that
/// counts itself, the body).
fn whole_messages(bytes: &[u8]) -> usize {
    let mut at = 0;
    while let Some(len) = bytes.get(at + 1..at + 5) {
        let end = at + 1 + i32::from_be_bytes(len.try_into().unwrap()) as usize;
        if end > bytes.len() {
            break;
        }
        at = end;
    }
    at
}

#[allow(clippy::too_many_arguments)]
async fn relay(
    mut client: TcpStream,
    upstream: &str,
    sent: Arc<Mutex<Vec<Sent>>>,
    dropped: Arc<Mutex<usize>>,
    executes: Arc<Mutex<Vec<i32>>>,
    faults: Faults,
    held: Arc<watch::Sender<bool>>,
    rewords: Rewords,
) -> std::io::Result<()> {
    let mut body = read_startup(&mut client).await?;
    // SSLRequest / GSSENCRequest: "no", then the real startup message.
    while body.len() == 4 && matches!(i32::from_be_bytes(body[..4].try_into().unwrap()), 80877103 | 80877104) {
        client.write_all(b"N").await?;
        body = read_startup(&mut client).await?;
    }
    let mut server = TcpStream::connect(upstream).await?;
    let code = i32::from_be_bytes(body[..4].try_into().unwrap());
    if code == 80877102 {
        // A cancel request: passed on as it is.
        server.write_i32(body.len() as i32 + 4).await?;
        server.write_all(&body).await?;
        return Ok(());
    }
    // The startup parameters: `key\0value\0…\0`, without `options`.
    let mut out = body[..4].to_vec();
    let mut parts = body[4..].split(|b| *b == 0).collect::<Vec<_>>();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    for pair in parts.chunks(2) {
        if pair[0] == b"options" {
            *dropped.lock().unwrap() += 1;
            continue;
        }
        for p in pair {
            out.extend_from_slice(p);
            out.push(0);
        }
    }
    out.push(0);
    server.write_i32(out.len() as i32 + 4).await?;
    server.write_all(&out).await?;
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    tokio::spawn(async move {
        let mut buf = vec![0; 16 * 1024];
        let mut pending = Vec::new();
        let mut gate = held.subscribe();
        while let Ok(n) = sr.read(&mut buf).await {
            pending.extend_from_slice(&buf[..n]);
            let _ = gate.wait_for(|held| !held).await;
            // Whole messages only, so what is reworded is never cut in two; at the end, what came.
            let whole = if n == 0 { pending.len() } else { whole_messages(&pending) };
            let mut out: Vec<u8> = pending.drain(..whole).collect();
            for (from, to) in rewords.lock().unwrap().iter() {
                while let Some(at) = out.windows(from.len()).position(|w| w == from.as_slice()) {
                    out[at..at + to.len()].copy_from_slice(to);
                }
            }
            if cw.write_all(&out).await.is_err() || n == 0 {
                break;
            }
        }
        let _ = cw.shutdown().await;
    });
    while let Ok(tag) = cr.read_u8().await {
        let len = cr.read_i32().await? as usize;
        let mut msg = vec![0; len - 4];
        cr.read_exact(&mut msg).await?;
        let cstr = |b: &[u8]| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or(&[])).to_string();
        let fault = {
            let mut faults = faults.lock().unwrap();
            let met = faults.iter().position(|(wanted, _)| tag == b'Q' && wanted.is(&cstr(&msg)));
            met.map(|at| faults.remove(at))
        };
        let msg = match fault {
            Some((_, Fault::Close)) => {
                let _ = sw.shutdown().await;
                return Ok(());
            }
            Some((_, Fault::Instead(text))) => [text.as_bytes(), &[0]].concat(),
            Some((Wanted::Exactly(start) | Wanted::Starting(start), Fault::Head(text))) => {
                [text.as_bytes(), &msg[start.len()..]].concat()
            }
            Some((_, Fault::Delay(by))) => {
                tokio::time::sleep(by).await;
                msg
            }
            None => msg,
        };
        let len = msg.len() + 4;
        match tag {
            b'Q' => sent.lock().unwrap().push(Sent::Query(cstr(&msg))),
            b'P' => {
                // The statement's name, then its text.
                let name_end = msg.iter().position(|b| *b == 0).unwrap_or(0);
                sent.lock().unwrap().push(Sent::Parse(cstr(&msg[name_end + 1..])));
            }
            b'E' => {
                // The portal's name, then the rows to return.
                let name_end = msg.iter().position(|b| *b == 0).unwrap_or(0);
                if let Some(n) = msg.get(name_end + 1..name_end + 5) {
                    executes.lock().unwrap().push(i32::from_be_bytes(n.try_into().unwrap()));
                }
            }
            _ => {}
        }
        sw.write_u8(tag).await?;
        sw.write_i32(len as i32).await?;
        sw.write_all(&msg).await?;
        if tag == b'X' {
            break;
        }
    }
    let _ = sw.shutdown().await;
    Ok(())
}

/// Check what a session in a schema of its own sent behind the proxy: nothing
/// sets `search_path` for the session (no `SET search_path`, `SET SESSION search_path` or
/// `set_config()` at all), every transaction block (a chained one too) has `SET LOCAL
/// search_path …` before it ends, and every statement that names `table` runs where the path is set (a simple query
/// that set it first, or a block that did). Returns how many blocks it saw.
pub fn check_wire(sent: &[Sent], table: &str) -> usize {
    let (mut block, mut path, mut blocks) = (false, false, 0);
    let mut step = |sql: &str, block: &mut bool, path: &mut bool| {
        let u = sql.trim().to_uppercase();
        assert!(!u.contains("SET_CONFIG"), "set_config() sent: {sql}");
        if u.starts_with("SET") && u.contains("SEARCH_PATH") {
            assert!(u.starts_with("SET LOCAL SEARCH_PATH TO "), "a session-level SET: {sql}");
            *path = true;
        } else if u.starts_with("BEGIN") || u.starts_with("START TRANSACTION") {
            (*block, *path) = (true, false);
        } else if u.starts_with("COMMIT") || u.starts_with("END") || (u.starts_with("ROLLBACK") && !u.contains(" TO "))
        {
            if *block {
                assert!(*path, "a block ended without SET LOCAL search_path: {sql}");
                blocks += 1;
            }
            // `AND CHAIN` opens a new block at once (`AND NO CHAIN` does not).
            let chain = u.contains(" AND CHAIN");
            (*block, *path) = (*block && chain, false);
        } else if u.contains(&table.to_uppercase()) {
            assert!(*path, "`{sql}` ran without the path");
        }
    };
    for s in sent {
        match s {
            Sent::Parse(sql) => step(sql, &mut block, &mut path),
            Sent::Query(text) => {
                for sql in text.split(';').filter(|p| !p.trim().is_empty()) {
                    step(sql, &mut block, &mut path);
                }
                // A simple query outside a block is a transaction of its own.
                if !block {
                    path = false;
                }
            }
        }
    }
    blocks
}
