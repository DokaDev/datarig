//! Host keys: the keys the user already
//! trusts in OpenSSH (`~/.ssh/known_hosts`, read only, never written) and datarig's own file
//! (`<data dir>/known_hosts`, mode 0600), which is the only one datarig writes. For a host
//! with entries in datarig's file those decide; otherwise OpenSSH's do.
//!
//! The format is OpenSSH's: `hosts keytype base64 [comment]`, hosts a comma-separated list of
//! names or `[name]:port`, `*` and `?` wildcards, `!` negations and hashed names
//! (`|1|salt|hash`, HMAC-SHA1). A `@revoked` line refuses its key for the hosts it names;
//! `@cert-authority` lines are skipped (host certificates are not used). Lines datarig cannot
//! read (a key type it does not know) are skipped, as OpenSSH skips them.
//!
//! A host whose entries do not include the presented key (of any type) has **changed**, never
//! "unknown": the caller warns and asks, and nothing is accepted by itself.

use datarig_core::fault::Fault;
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::{HashAlg, PublicKey};
use sha1::Sha1;
use std::io;
use std::path::{Path, PathBuf};

/// The files the keys come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownHosts {
    /// OpenSSH's file of the user (`~/.ssh/known_hosts`), read only; `None` when there is no
    /// home directory.
    pub user: Option<PathBuf>,
    /// datarig's own file, the only one written.
    pub app: PathBuf,
}

/// A key a file has for a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub file: PathBuf,
    /// 1-based.
    pub line: usize,
    /// `ssh-ed25519`, `rsa-sha2-512`, … as OpenSSH names the key type.
    pub algorithm: String,
    /// `SHA256:…` as `ssh-keygen -lf` prints it.
    pub fingerprint: String,
}

/// What the files say about a host's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// One of its entries is this key.
    Known,
    /// No file has an entry for the host.
    Unknown,
    /// The host has entries, none of them this key: the keys stored for it (from datarig's
    /// file when it has any, else from OpenSSH's).
    Changed(Vec<Stored>),
    /// A `@revoked` line names this key for the host: never trusted.
    Revoked(Stored),
}

/// A file could not be read or written: the host's keys are not known (never "no keys").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub file: PathBuf,
    pub fault: Fault,
}

/// `SHA256:…` of `key`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// How the host is named in a known_hosts file: `name` on port 22, else `[name]:port`.
pub fn host_spec(host: &str, port: u16) -> String {
    if port == 22 { host.to_string() } else { format!("[{host}]:{port}") }
}

/// One usable line of a file.
struct Entry {
    line: usize,
    revoked: bool,
    patterns: String,
    key: PublicKey,
}

impl KnownHosts {
    /// What the files say about `key` for `host:port`.
    pub fn check(&self, host: &str, port: u16, key: &PublicKey) -> Result<Verdict, FileError> {
        let spec = host_spec(host, port);
        let app = read(&self.app)?;
        let user = match &self.user {
            Some(p) => read(p)?,
            None => Vec::new(),
        };
        for (file, entries) in [(&self.app, &app), (self.user.as_ref().unwrap_or(&self.app), &user)] {
            if let Some(e) = entries.iter().find(|e| e.revoked && &e.key == key && matches(&spec, &e.patterns)) {
                return Ok(Verdict::Revoked(stored(file, e)));
            }
        }
        let of = |entries: &[Entry]| -> Vec<usize> {
            (0..entries.len()).filter(|i| !entries[*i].revoked && matches(&spec, &entries[*i].patterns)).collect()
        };
        let (file, entries, found) = match of(&app) {
            found if !found.is_empty() => (&self.app, &app, found),
            _ => match &self.user {
                Some(p) => (p, &user, of(&user)),
                None => return Ok(Verdict::Unknown),
            },
        };
        if found.is_empty() {
            Ok(Verdict::Unknown)
        } else if found.iter().any(|i| &entries[*i].key == key) {
            Ok(Verdict::Known)
        } else {
            Ok(Verdict::Changed(found.iter().map(|i| stored(file, &entries[*i])).collect()))
        }
    }

    /// The key types stored for the host (datarig's file first), most recent first: offered
    /// first in the key exchange, so a server with several keys shows the one that is known.
    pub fn algorithms(&self, host: &str, port: u16) -> Vec<russh::keys::Algorithm> {
        let spec = host_spec(host, port);
        let mut out = Vec::new();
        for path in std::iter::once(&self.app).chain(self.user.as_ref()) {
            for e in read(path).unwrap_or_default().iter().rev() {
                let a = e.key.algorithm();
                if !e.revoked && matches(&spec, &e.patterns) && !out.contains(&a) {
                    out.push(a);
                }
            }
        }
        out
    }

    /// Trust `key` for `host:port`: datarig's file keeps only this key for the host (its other
    /// lines stay as they are), written whole and private. OpenSSH's file is never touched.
    pub fn trust(&self, host: &str, port: u16, key: &PublicKey) -> Result<(), FileError> {
        let spec = host_spec(host, port);
        let fail = |e: &io::Error| FileError { file: self.app.clone(), fault: Fault::io_at(e, &self.app) };
        let text = match std::fs::read_to_string(&self.app) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(fail(&e)),
        };
        let mut out = String::new();
        for line in text.lines() {
            if parse(line).is_some_and(|(revoked, patterns, _)| !revoked && patterns.split(',').any(|p| p == spec)) {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        let openssh =
            key.to_openssh().map_err(|e| FileError { file: self.app.clone(), fault: Fault::other(e.to_string()) })?;
        let mut words = openssh.split_whitespace();
        let (kind, data) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
        out.push_str(&format!("{spec} {kind} {data}\n"));
        if let Some(dir) = self.app.parent() {
            std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
        }
        datarig_core::fsutil::atomic_write_private(&self.app, out.as_bytes()).map_err(|e| fail(&e))
    }
}

fn stored(file: &Path, e: &Entry) -> Stored {
    Stored {
        file: file.to_path_buf(),
        line: e.line,
        algorithm: e.key.algorithm().to_string(),
        fingerprint: fingerprint(&e.key),
    }
}

/// The usable lines of a file; a file that is not there has none. One that cannot be read is
/// an error.
fn read(path: &Path) -> Result<Vec<Entry>, FileError> {
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(FileError { file: path.to_path_buf(), fault: Fault::io_at(&e, path) }),
    };
    Ok(text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let (revoked, patterns, key) = parse(l)?;
            Some(Entry { line: i + 1, revoked, patterns: patterns.to_string(), key: key? })
        })
        .collect())
}

/// A line: whether it is `@revoked`, its host patterns and its key (`None`: a key datarig
/// cannot read). `None` for comments, blank lines, `@cert-authority` and other markers.
fn parse(line: &str) -> Option<(bool, &str, Option<PublicKey>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut words = line.split_whitespace();
    let mut first = words.next()?;
    let mut revoked = false;
    if let Some(marker) = first.strip_prefix('@') {
        if marker != "revoked" {
            return None;
        }
        revoked = true;
        first = words.next()?;
    }
    let (kind, data) = (words.next()?, words.next()?);
    Some((revoked, first, PublicKey::from_openssh(&format!("{kind} {data}")).ok()))
}

/// Whether `spec` (`name` or `[name]:port`) matches a comma-separated pattern list: a negated
/// pattern that matches excludes it, else any other pattern that matches includes it.
fn matches(spec: &str, patterns: &str) -> bool {
    let mut hit = false;
    for p in patterns.split(',') {
        if let Some(neg) = p.strip_prefix('!') {
            if one(spec, neg) {
                return false;
            }
        } else if one(spec, p) {
            hit = true;
        }
    }
    hit
}

fn one(spec: &str, pattern: &str) -> bool {
    if let Some(hashed) = pattern.strip_prefix("|1|") {
        let mut parts = hashed.split('|');
        let (Some(salt), Some(hash)) = (parts.next(), parts.next()) else { return false };
        let b64 = data_encoding::BASE64;
        let (Ok(salt), Ok(hash)) = (b64.decode(salt.as_bytes()), b64.decode(hash.as_bytes())) else { return false };
        let Ok(mac) = Hmac::<Sha1>::new_from_slice(&salt) else { return false };
        return mac.chain_update(spec.as_bytes()).verify_slice(&hash).is_ok();
    }
    glob(spec.as_bytes(), pattern.as_bytes())
}

/// OpenSSH's host pattern: `*` any run, `?` one character; case-insensitive like host names.
fn glob(s: &[u8], p: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, _) => s.is_empty(),
        (Some(b'*'), _) => glob(s, &p[1..]) || (!s.is_empty() && glob(&s[1..], p)),
        (Some(b'?'), Some(_)) => glob(&s[1..], &p[1..]),
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => glob(&s[1..], &p[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
