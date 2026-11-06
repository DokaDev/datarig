//! The proxy an update check goes through, from the environment as other command-line programs
//! read it: `HTTPS_PROXY`, else `ALL_PROXY`, and `NO_PROXY` (host names separated by commas, each
//! matching itself and its subdomains; `*` for every host). Each is also read in lower case, the
//! upper case first; the caller reads them ([`Vars`]).
//!
//! Only an HTTP proxy (`http://[user:password@]host[:port]`, or the same without `http://`) is
//! used. Any other kind (SOCKS, HTTPS, an address that cannot be read) means no check at all:
//! never a direct connection the proxy setting was there to avoid.

/// The proxy variables of the environment (an empty value counts as not set).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vars {
    /// `HTTPS_PROXY`, else `https_proxy`.
    pub https_proxy: Option<String>,
    /// `ALL_PROXY`, else `all_proxy`.
    pub all_proxy: Option<String>,
    /// `NO_PROXY`, else `no_proxy`.
    pub no_proxy: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proxy {
    /// No proxy for this host.
    Direct,
    /// Through this HTTP proxy, as `http://…`.
    Http(String),
    /// A proxy is set but is not one that can be used: no check.
    Unsupported,
}

/// The proxy for a request to `host`.
pub fn for_host(vars: &Vars, host: &str) -> Proxy {
    fn set(v: &Option<String>) -> Option<&str> {
        v.as_deref().map(str::trim).filter(|v| !v.is_empty())
    }
    let Some(url) = set(&vars.https_proxy).or_else(|| set(&vars.all_proxy)) else {
        return Proxy::Direct;
    };
    if set(&vars.no_proxy).is_some_and(|list| bypassed(list, host)) {
        return Proxy::Direct;
    }
    http_proxy(url).map_or(Proxy::Unsupported, Proxy::Http)
}

/// Whether `NO_PROXY`'s `list` covers `host`.
fn bypassed(list: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    list.split(',').map(str::trim).filter(|e| !e.is_empty()).any(|entry| {
        if entry == "*" {
            return true;
        }
        let entry = entry.to_ascii_lowercase();
        // A port on the entry (`example.com:443`) does not narrow it.
        let name =
            entry.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(&*entry, |(n, _)| n);
        let name = name.trim_start_matches("*.").trim_start_matches('.');
        !name.is_empty() && (host == name || host.ends_with(&format!(".{name}")))
    })
}

/// `url` as `http://…` when it is an HTTP proxy with a host.
fn http_proxy(url: &str) -> Option<String> {
    let rest = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some(_) => return None,
        None => url,
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // `[v6 address]` or a name, then `:port` or nothing.
    let (host, port) = match host_port.strip_prefix('[') {
        Some(v6) => {
            let (addr, after) = v6.split_once(']')?;
            (addr, if after.is_empty() { None } else { Some(after.strip_prefix(':')?) })
        }
        None => match host_port.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        },
    };
    let port_ok = port.is_none_or(|p| p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok());
    let host_ok = !host.is_empty() && !host.contains(|c: char| c.is_whitespace() || "[]@/".contains(c));
    let ok = host_ok && port_ok && rest[authority.len()..].trim_matches('/').is_empty();
    ok.then(|| format!("http://{authority}"))
}

#[cfg(test)]
mod tests;
