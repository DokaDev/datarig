//! The update check's one request: `GET https://api.github.com/repos/DokaDev/datarig/releases/latest`
//! ([`datarig_core::update::URL`]), with nothing about the user in it. The decisions (when to
//! ask, what the answer means for the screen) are `datarig_core::update`'s; this crate only asks.
//!
//! The request carries exactly `User-Agent: datarig`, `Accept: application/vnd.github+json`,
//! `X-GitHub-Api-Version` and, after a first answer, `If-None-Match` with GitHub's tag for it
//! (ureq adds `Host`). No `Accept-Encoding` (the answer comes as it is), no cookies, no token, no
//! redirects. The server's certificate is checked by the platform's verifier, so a company's own
//! root that the system trusts is trusted too, and nothing is ever trusted that the system does
//! not. The proxy is the one [`datarig_core::update::proxy::for_host`] chose from the
//! environment, and only that one: ureq's own reading of the environment is not used.
//!
//! The call blocks (at most [`Timeouts::total`]); the caller runs it on a thread of its own, so
//! nothing ever waits for it. One attempt: no retries.

use datarig_core::update::proxy::Proxy;
use datarig_core::update::release::MAX_ANSWER;
use datarig_core::update::{Answer, Outcome, Release, URL, USER_AGENT};
use std::sync::Arc;
use std::time::Duration;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

/// The version of GitHub's REST API asked for.
pub const API_VERSION: &str = "2022-11-28";

/// What is asked for.
pub const ACCEPT: &str = "application/vnd.github+json";

/// Asks for the latest release. The app holds one; tests give it a fake.
pub trait Fetch: Send + Sync {
    /// The latest release, or why it is not known. `etag`: GitHub's tag for the last answer.
    fn fetch(&self, etag: Option<&str>) -> Answer;
}

/// How long the request may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// The name looked up and the connection made (to the proxy, when there is one).
    pub connect: Duration,
    /// The whole exchange.
    pub total: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts { connect: Duration::from_secs(3), total: Duration::from_secs(8) }
    }
}

/// The request over HTTPS.
pub struct Https {
    agent: ureq::Agent,
    url: String,
}

impl Https {
    /// To GitHub through `proxy`; `None` when the proxy is one that cannot be used (no check
    /// then, never a direct connection).
    pub fn new(proxy: &Proxy) -> Option<Self> {
        Self::with(URL, proxy, RootCerts::PlatformVerifier, Timeouts::default())
    }

    /// To `url`, trusting `roots` (the tests' own CA), with `timeouts`.
    pub fn with(url: &str, proxy: &Proxy, roots: RootCerts, timeouts: Timeouts) -> Option<Self> {
        let proxy = match proxy {
            Proxy::Direct => None,
            Proxy::Http(p) => Some(ureq::Proxy::new(p).ok()?),
            Proxy::Unsupported => return None,
        };
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .root_certs(roots)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .proxy(proxy)
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .accept(ACCEPT)
            .accept_encoding("")
            .max_idle_connections(0)
            .timeout_resolve(Some(timeouts.connect))
            .timeout_connect(Some(timeouts.connect))
            .timeout_global(Some(timeouts.total))
            .build()
            .new_agent();
        Some(Https { agent, url: url.to_string() })
    }

    /// Trusting only `ders` (DER certificates).
    pub fn roots(ders: &[Vec<u8>]) -> RootCerts {
        RootCerts::new_with_certs(&ders.iter().map(|d| Certificate::from_der(d).to_owned()).collect::<Vec<_>>())
    }
}

impl Fetch for Https {
    fn fetch(&self, etag: Option<&str>) -> Answer {
        let mut req = self.agent.get(&self.url).header("X-GitHub-Api-Version", API_VERSION);
        if let Some(e) = etag {
            req = req.header("If-None-Match", e);
        }
        let mut res = match req.call() {
            Ok(r) => r,
            Err(e) => return Answer::Failed(outcome(&e)),
        };
        let status = res.status().as_u16();
        match status {
            200 => {}
            304 => return Answer::NotModified,
            403 | 429 => return Answer::Failed(Outcome::RateLimited),
            407 => return Answer::Failed(Outcome::Proxy),
            500..=599 => return Answer::Failed(Outcome::Unreachable),
            _ => return Answer::Failed(Outcome::BadAnswer),
        }
        let tag = res.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
        // One byte past the cap tells an answer that is too long from one that just fits.
        let body = match res.body_mut().with_config().limit(MAX_ANSWER as u64 + 1).read_to_vec() {
            Ok(b) => b,
            Err(ureq::Error::BodyExceedsLimit(_)) => return Answer::Failed(Outcome::BadAnswer),
            Err(e) => return Answer::Failed(outcome(&e)),
        };
        match Release::from_json(&body) {
            Ok(r) => Answer::Release(r, tag),
            Err(_) => Answer::Failed(Outcome::BadAnswer),
        }
    }
}

/// How a failed request ended.
pub fn outcome(e: &ureq::Error) -> Outcome {
    use ureq::Error as E;
    match e {
        E::Timeout(_) => Outcome::TimedOut,
        E::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => Outcome::TimedOut,
        // A certificate the handshake refused comes as an I/O error around rustls's.
        E::Io(io) if io.get_ref().is_some_and(|i| i.downcast_ref::<rustls::Error>().is_some()) => Outcome::Tls,
        E::Io(_) | E::HostNotFound | E::ConnectionFailed => Outcome::Unreachable,
        E::Tls(_) | E::Rustls(_) | E::Pem(_) | E::TlsRequired => Outcome::Tls,
        E::ConnectProxyFailed(_) | E::InvalidProxyUrl => Outcome::Proxy,
        E::BodyExceedsLimit(_) | E::Protocol(_) | E::LargeResponseHeader(..) => Outcome::BadAnswer,
        _ => Outcome::Unreachable,
    }
}

#[cfg(test)]
mod tests;
