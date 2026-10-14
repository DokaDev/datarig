//! A connection pooler's part in the tests: an in-process TCP proxy in front of
//! the test server that drops the `options` parameter from every startup message, as a pooler
//! does (PgBouncer with `ignore_startup_parameters = options`), and records what each client
//! sends: the text of every simple `Query` and every `Parse`, in the order they go out. It
//! answers `SSLRequest`/`GSSENCRequest` with "no" and passes everything else through
//! unchanged (authentication included); a cancel request goes to the server as it is.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A message a client sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// A simple query (`Q`): its text, maybe several statements.
    Query(String),
    /// A `Parse` (`P`): its statement's text.
    Parse(String),
}

pub struct Proxy {
    /// `host:port` to connect to instead of the server.
    pub addr: String,
    /// What the clients sent, in order (all of them together).
    pub sent: Arc<Mutex<Vec<Sent>>>,
    /// How many startup messages had `options` (dropped).
    pub dropped: Arc<Mutex<usize>>,
}

impl Proxy {
    /// Listen on a free local port and relay to `upstream` (`host:port`).
    pub async fn start(upstream: &str) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap().to_string();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(Mutex::new(0));
        let (s, d, up) = (sent.clone(), dropped.clone(), upstream.to_string());
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (s, d, up) = (s.clone(), d.clone(), up.clone());
                tokio::spawn(async move {
                    let _ = relay(client, &up, s, d).await;
                });
            }
        });
        Proxy { addr, sent, dropped }
    }

    /// What the clients sent so far.
    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
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

async fn relay(
    mut client: TcpStream,
    upstream: &str,
    sent: Arc<Mutex<Vec<Sent>>>,
    dropped: Arc<Mutex<usize>>,
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
        let _ = tokio::io::copy(&mut sr, &mut cw).await;
        let _ = cw.shutdown().await;
    });
    while let Ok(tag) = cr.read_u8().await {
        let len = cr.read_i32().await? as usize;
        let mut msg = vec![0; len - 4];
        cr.read_exact(&mut msg).await?;
        let cstr = |b: &[u8]| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or(&[])).to_string();
        match tag {
            b'Q' => sent.lock().unwrap().push(Sent::Query(cstr(&msg))),
            b'P' => {
                // The statement's name, then its text.
                let name_end = msg.iter().position(|b| *b == 0).unwrap_or(0);
                sent.lock().unwrap().push(Sent::Parse(cstr(&msg[name_end + 1..])));
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
