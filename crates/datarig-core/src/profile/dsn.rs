//! PostgreSQL connection URLs (`postgres://user:pass@host:port/db?k=v`).
//!
//! Only the URL form is handled here (the profile form keeps it in sync with the individual
//! fields). Userinfo, database and query values are percent-decoded on parse and encoded on
//! format; IPv6 hosts are written in brackets. Multi-host URLs are rejected.

use crate::i18n::{Label, Msg};
use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Dsn {
    pub user: String,
    pub password: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    /// Query parameters in order (`sslmode`, …), decoded.
    pub params: Vec<(String, String)>,
}

/// Manual impl (instead of `#[derive(Debug)]`) so `{:?}` never prints the plaintext password.
impl fmt::Debug for Dsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self.password.as_deref().map(|p| if p.is_empty() { "<unset>" } else { "<redacted>" });
        f.debug_struct("Dsn")
            .field("user", &self.user)
            .field("password", &redacted)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("params", &self.params)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DsnError {
    /// Does not start with `postgres://` or `postgresql://`.
    Scheme,
    /// Port is not a number in 1..=65535.
    Port(String),
    /// Malformed `%XX` escape or escape that is not UTF-8.
    Encoding,
    /// Unbalanced `[` / `]`, several hosts, or a bare IPv6 address.
    Host(String),
}

impl DsnError {
    /// Catalog message describing the error.
    pub fn message(&self) -> Msg {
        match self {
            DsnError::Scheme => Label::DsnErrScheme.into(),
            DsnError::Port(p) => Msg::DsnErrPort { value: p.clone() },
            DsnError::Encoding => Label::DsnErrEncoding.into(),
            DsnError::Host(h) => Msg::DsnErrHost { value: h.clone() },
        }
    }
}

impl Dsn {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix))
        .then(|| &s[prefix.len()..])
}

fn decode(s: &str) -> Result<String, DsnError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3).ok_or(DsnError::Encoding)?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| DsnError::Encoding)?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| DsnError::Encoding)
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn parse_port(p: &str) -> Result<Option<u16>, DsnError> {
    if p.is_empty() {
        return Err(DsnError::Port(String::new()));
    }
    match p.parse::<u16>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(DsnError::Port(p.to_string())),
    }
}

pub fn parse(input: &str) -> Result<Dsn, DsnError> {
    let s = input.trim();
    let rest =
        strip_prefix_ci(s, "postgres://").or_else(|| strip_prefix_ci(s, "postgresql://")).ok_or(DsnError::Scheme)?;
    let (main, query) = match rest.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match main.find('/') {
        Some(i) => (&main[..i], &main[i + 1..]),
        None => (main, ""),
    };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let mut d = Dsn::default();
    if let Some(u) = userinfo {
        match u.split_once(':') {
            Some((user, pw)) => {
                d.user = decode(user)?;
                d.password = Some(decode(pw)?);
            }
            None => d.user = decode(u)?,
        }
    }
    if let Some(v6) = hostport.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or_else(|| DsnError::Host(hostport.to_string()))?;
        if h.is_empty() {
            return Err(DsnError::Host(hostport.to_string()));
        }
        d.host = h.to_string();
        if let Some(p) = after.strip_prefix(':') {
            d.port = parse_port(p)?;
        } else if !after.is_empty() {
            return Err(DsnError::Host(hostport.to_string()));
        }
    } else {
        let (h, p) = match hostport.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        };
        if h.contains([',', ']', '[']) || p.is_some_and(|p| p.contains(':')) {
            return Err(DsnError::Host(hostport.to_string()));
        }
        d.host = decode(h)?;
        if let Some(p) = p {
            d.port = parse_port(p)?;
        }
    }
    d.database = decode(path)?;
    if let Some(q) = query {
        for pair in q.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            d.params.push((decode(k)?, decode(v)?));
        }
    }
    Ok(d)
}

pub fn format(d: &Dsn) -> String {
    let mut s = String::from("postgres://");
    if !d.user.is_empty() || d.password.is_some() {
        s.push_str(&encode(&d.user));
        if let Some(pw) = &d.password {
            s.push(':');
            s.push_str(&encode(pw));
        }
        s.push('@');
    }
    if d.host.contains(':') {
        s.push_str(&format!("[{}]", d.host));
    } else {
        s.push_str(&d.host);
    }
    if let Some(p) = d.port {
        s.push_str(&format!(":{p}"));
    }
    if !d.database.is_empty() || !d.params.is_empty() {
        s.push('/');
        s.push_str(&encode(&d.database));
    }
    for (i, (k, v)) in d.params.iter().enumerate() {
        s.push(if i == 0 { '?' } else { '&' });
        s.push_str(&encode(k));
        s.push('=');
        s.push_str(&encode(v));
    }
    s
}

/// Byte ranges of `text` that must not be shown: the password part of the userinfo
/// (`user:<secret>@host`). Used to mask the DSN field while the user is typing one.
pub fn secret_span(text: &str) -> Option<(usize, usize)> {
    let t = text.trim_start();
    let lead = text.len() - t.len();
    let scheme_end = t.find("://")? + 3;
    let rest = &t[scheme_end..];
    let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let at = rest[..auth_end].rfind('@')?;
    let colon = rest[..at].find(':')?;
    Some((lead + scheme_end + colon + 1, lead + scheme_end + at))
}

#[cfg(test)]
mod tests;
