//! TLS as the rest of the app sees it: a profile's settings ([`TlsMode`], [`TlsSettings`]), why
//! a TLS connection could not be made ([`TlsProblem`]) and what a connection ended up with
//! ([`Encryption`]). The handshake itself is the `datarig-tls` crate's.

use crate::fault::Fault;
use crate::profile::dsn::Scheme;

/// How a connection uses TLS. Each driver spells the modes as its own client does
/// ([`TlsMode::name`]): PostgreSQL's `sslmode`, MySQL's `--ssl-mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TlsMode {
    /// Never encrypted.
    Disable,
    /// Encrypted when the server offers TLS, without checking its certificate; a server that
    /// does not offer it gets a plain connection (which the app shows as such). A server that
    /// offers TLS and then fails the handshake is an error, never a plain connection.
    Prefer,
    /// Always encrypted, without checking the server's certificate (with a CA file: as
    /// [`TlsMode::VerifyCa`]).
    Require,
    /// Encrypted, and the server's certificate chain checked against the CA file (not its
    /// name).
    VerifyCa,
    /// Encrypted, the chain checked (against the CA file, else the system's trusted
    /// certificates) and the certificate checked to be the profile's host's.
    VerifyFull,
}

impl TlsMode {
    /// Every mode, from the weakest.
    pub const ALL: [TlsMode; 5] =
        [TlsMode::Disable, TlsMode::Prefer, TlsMode::Require, TlsMode::VerifyCa, TlsMode::VerifyFull];

    /// The mode's name for a profile of `scheme`: libpq's `sslmode` values for PostgreSQL,
    /// the mysql client's `--ssl-mode` values for MySQL.
    pub fn name(self, scheme: Scheme) -> &'static str {
        match (scheme, self) {
            (Scheme::Postgres, TlsMode::Disable) => "disable",
            (Scheme::Postgres, TlsMode::Prefer) => "prefer",
            (Scheme::Postgres, TlsMode::Require) => "require",
            (Scheme::Postgres, TlsMode::VerifyCa) => "verify-ca",
            (Scheme::Postgres, TlsMode::VerifyFull) => "verify-full",
            (Scheme::MySql, TlsMode::Disable) => "DISABLED",
            (Scheme::MySql, TlsMode::Prefer) => "PREFERRED",
            (Scheme::MySql, TlsMode::Require) => "REQUIRED",
            (Scheme::MySql, TlsMode::VerifyCa) => "VERIFY_CA",
            (Scheme::MySql, TlsMode::VerifyFull) => "VERIFY_IDENTITY",
        }
    }

    /// The mode `text` names for a profile of `scheme` (MySQL's names in any case, as its
    /// client reads them); `None` for anything else.
    pub fn parse(scheme: Scheme, text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| match scheme {
            Scheme::Postgres => m.name(scheme) == text,
            Scheme::MySql => m.name(scheme).eq_ignore_ascii_case(text),
        })
    }

    /// Its place in [`TlsMode::ALL`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// The mode of a profile of `scheme` that names none. PostgreSQL: `disable`, as a profile
    /// without `sslmode` has always connected (the profile form writes `prefer` for a new one).
    /// MySQL: `PREFERRED`, the mysql client's default — except for a profile that pins the
    /// server's public key (`pinned_key`): its password goes encrypted with that key, which a
    /// TLS connection does not do, so it stays unencrypted (`DISABLED`) until a mode is chosen.
    pub fn default_for(scheme: Scheme, pinned_key: bool) -> Self {
        match scheme {
            Scheme::Postgres => TlsMode::Disable,
            Scheme::MySql if pinned_key => TlsMode::Disable,
            Scheme::MySql => TlsMode::Prefer,
        }
    }

    /// Whether the server's certificate is checked.
    pub fn verifies(self) -> bool {
        matches!(self, TlsMode::VerifyCa | TlsMode::VerifyFull)
    }
}

/// A file of a profile's TLS settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsFile {
    /// The certificates the server's chain is checked against (`sslrootcert`).
    Ca,
    /// The client's certificate (`sslcert`).
    Cert,
    /// The client certificate's private key (`sslkey`).
    Key,
}

impl TlsFile {
    /// The profile's key for the file (also the PostgreSQL URL parameter).
    pub fn key(self) -> &'static str {
        match self {
            TlsFile::Ca => "sslrootcert",
            TlsFile::Cert => "sslcert",
            TlsFile::Key => "sslkey",
        }
    }
}

/// A profile's TLS settings, checked: the mode it connects in and its files (paths as the
/// profile names them, absolute or `~/…`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsSettings {
    pub mode: TlsMode,
    pub ca_file: Option<String>,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
}

impl TlsSettings {
    /// No TLS.
    pub fn plain() -> Self {
        Self { mode: TlsMode::Disable, ca_file: None, cert_file: None, key_file: None }
    }

    /// The settings of a profile of `scheme` whose mode is written `mode` (empty: none, see
    /// [`TlsMode::default_for`]) with these files (empty: none). As libpq does, `require` with
    /// a CA file checks the chain (`verify-ca`), and the CA file `system` (libpq 16) means the
    /// system's trusted certificates and checks the name too (`verify-full`). `verify-ca` needs
    /// a CA file (there is nothing else a chain without a name could be checked against). A
    /// client certificate needs its key and the other way round. Paths must be absolute or start
    /// with `~/`. With TLS off (`disable`) the files are not used, nor checked.
    ///
    /// For MySQL, `prefer` and `disable` count as no mode: the profile form wrote them into MySQL
    /// profiles before MySQL had TLS, and the MySQL driver never read them. The other libpq
    /// spellings are read as the MySQL mode they mean, never as a weaker one.
    pub fn new(scheme: Scheme, mode: &str, files: [Option<&str>; 3], pinned_key: bool) -> Result<Self, TlsProblem> {
        let text = mode.trim();
        let mut mode = match TlsMode::parse(scheme, text) {
            _ if text.is_empty() => TlsMode::default_for(scheme, pinned_key),
            Some(m) => m,
            None if scheme == Scheme::MySql => match TlsMode::parse(Scheme::Postgres, text) {
                Some(TlsMode::Disable | TlsMode::Prefer) => TlsMode::default_for(scheme, pinned_key),
                Some(m) => m,
                None => return Err(TlsProblem::UnknownMode(text.to_string())),
            },
            None => return Err(TlsProblem::UnknownMode(text.to_string())),
        };
        if mode == TlsMode::Disable {
            return Ok(Self::plain());
        }
        let file = |f: TlsFile, p: Option<&str>| -> Result<Option<String>, TlsProblem> {
            let Some(p) = p.map(str::trim).filter(|p| !p.is_empty()) else { return Ok(None) };
            match p.starts_with("~/") || std::path::Path::new(p).has_root() {
                true => Ok(Some(p.to_string())),
                false => Err(TlsProblem::PathRelative { file: f, path: p.to_string() }),
            }
        };
        let [ca, cert, key] = files;
        let system = ca.map(str::trim) == Some(SYSTEM_CA);
        let ca = ca.filter(|_| !system);
        let (ca_file, cert_file, key_file) =
            (file(TlsFile::Ca, ca)?, file(TlsFile::Cert, cert)?, file(TlsFile::Key, key)?);
        if system {
            mode = TlsMode::VerifyFull;
        }
        if mode == TlsMode::Require && ca_file.is_some() {
            mode = TlsMode::VerifyCa;
        }
        if mode == TlsMode::VerifyCa && ca_file.is_none() {
            return Err(TlsProblem::NeedsCaFile);
        }
        match (&cert_file, &key_file) {
            (Some(_), None) => return Err(TlsProblem::CertWithoutKey),
            (None, Some(_)) => return Err(TlsProblem::KeyWithoutCert),
            _ => {}
        }
        Ok(Self { mode, ca_file, cert_file, key_file })
    }
}

/// The CA file that names the system's trusted certificates (libpq's `sslrootcert=system`).
pub const SYSTEM_CA: &str = "system";

/// Why a file of the TLS settings cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileProblem {
    /// It cannot be read (why).
    Unreadable(Fault),
    /// It has no certificate in PEM (a CA file, a client certificate file).
    NoCertificate,
    /// It has no private key in PEM that can be used (PKCS#8, PKCS#1 RSA or SEC1 EC).
    NoKey,
    /// The private key is encrypted with a passphrase, which datarig does not ask for.
    Encrypted,
    /// The client certificate and key cannot be used together, or a certificate in it is not
    /// one (why, for the log).
    Invalid(Fault),
}

/// Why a TLS connection was not made (or not used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TlsProblem {
    /// The profile's mode is not one its driver knows (as written).
    UnknownMode(String),
    /// The profile is kept as a DSN that cannot be read for its TLS settings (neither a URL nor
    /// a `key=value` string).
    DsnUnreadable,
    /// `verify-ca` / `VERIFY_CA` without a CA file.
    NeedsCaFile,
    /// A client certificate without its key.
    CertWithoutKey,
    /// A client key without its certificate.
    KeyWithoutCert,
    /// A file's path is relative (as written): it must be absolute or start with `~/`.
    PathRelative { file: TlsFile, path: String },
    /// A file cannot be used (its path as the profile names it).
    File { file: TlsFile, path: String, problem: FileProblem },
    /// The mode needs TLS and the server does not offer it.
    Unsupported,
    /// The server's certificate is not signed by a trusted certificate (the CA file's, or the
    /// system's), or its signature does not check out.
    Untrusted,
    /// The server's certificate is valid, and not for this host (the name checked).
    NameMismatch { host: String },
    /// The profile's host is neither a host name nor an IP address a certificate could name, so
    /// the server's certificate cannot be checked against it (as written).
    HostNotCheckable { host: String },
    /// The server's certificate is not meant for a server (its extended key usage says another
    /// purpose).
    NotForServer,
    /// The server's certificate (or one of its chain) has expired.
    Expired,
    /// The server's certificate (or one of its chain) is not valid yet.
    NotYetValid,
    /// The server's certificate was revoked.
    Revoked,
    /// The server refused the client certificate, or wanted one and got none.
    ClientCertRejected,
    /// `verify-full` without a CA file, and the system's trusted certificates cannot be read
    /// (why, for the log).
    SystemRoots(Fault),
    /// MySQL: the server asked for the password itself (a full `caching_sha2_password` login)
    /// over a TLS connection to another machine whose certificate was not checked, so it could
    /// be anyone's; nothing about the password was sent.
    PasswordUnverified,
    /// The handshake failed otherwise (why, for the log).
    Handshake(Fault),
}

impl TlsProblem {
    /// The raw text, for matching and the error log only.
    pub fn raw(&self) -> &str {
        match self {
            TlsProblem::UnknownMode(s) => s,
            TlsProblem::PathRelative { path, .. } => path,
            TlsProblem::File { problem: FileProblem::Unreadable(f) | FileProblem::Invalid(f), .. } => &f.detail,
            TlsProblem::Handshake(f) | TlsProblem::SystemRoots(f) => &f.detail,
            _ => "",
        }
    }
}

/// The TLS protocol version of a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

impl TlsVersion {
    /// `TLS 1.2`, `TLS 1.3`.
    pub fn label(self) -> &'static str {
        match self {
            TlsVersion::Tls12 => "TLS 1.2",
            TlsVersion::Tls13 => "TLS 1.3",
        }
    }
}

/// What was checked of the server's certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verified {
    /// Nothing (`prefer`, `require`): encrypted, but the server could be anyone.
    No,
    /// Its chain, against the CA file, not its name (`verify-ca`).
    Chain,
    /// Its chain and its name (`verify-full`).
    Full,
}

/// How a connection's traffic went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encryption {
    /// Not encrypted (by the connection itself; a tunnel may still carry it encrypted).
    #[default]
    Plain,
    /// Encrypted with TLS.
    Tls { version: TlsVersion, verified: Verified },
}

#[cfg(test)]
mod tests;
