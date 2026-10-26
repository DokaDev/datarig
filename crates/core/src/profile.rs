//! Connection profiles: the [`ConnectionConfig`] model
//! shared by the config file, the profile manager and the drivers, its stable [`ProfileId`], the
//! display attributes ([`color`], [`folder`]) and the `postgres://` [`dsn`] codec. Passwords
//! never live here at rest (see [`crate::secret`]; a profile only says where its password comes
//! from, [`ConnectionConfig::source`]); `Debug` output is redacted.

pub mod color;
pub mod dsn;
pub mod folder;
pub mod name;
pub mod session;
pub mod ssh;
pub mod tunnel;

use crate::secret::{PasswordSource, SourceKind};
use crate::tls::{TlsFile, TlsMode, TlsProblem, TlsSettings};
use serde::{Deserialize, Deserializer};
use std::fmt;
use uuid::Uuid;

/// A profile's stable internal id: a UUID v4 that never changes, unlike the name.
/// Keychain entries, `last_used` and (later) tabs and saved-query bindings refer to it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileId(Uuid);

impl ProfileId {
    /// A new random id.
    pub fn new() -> Self {
        ProfileId(Uuid::new_v4())
    }

    /// Placeholder of a profile read without an `id` (the config loader replaces it).
    pub(crate) fn unset() -> Self {
        ProfileId(Uuid::nil())
    }

    pub(crate) fn is_unset(&self) -> bool {
        self.0.is_nil()
    }

    /// A UUID in any standard text form; the nil UUID is not an id.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().filter(|u| !u.is_nil()).map(ProfileId)
    }

    /// The keychain account of this profile's password.
    pub fn account(&self) -> String {
        format!("profile:{self}")
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl fmt::Debug for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProfileId({self})")
    }
}

impl<'de> Deserialize<'de> for ProfileId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ProfileId::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("id = {s:?} is not a UUID")))
    }
}

/// PostgreSQL's `sslmode` values, in the order of [`TlsMode::ALL`].
pub const SSL_MODES: [&str; 5] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

/// `<redacted>` for a non-empty secret, `<unset>` for an empty one — never the value itself.
/// Used by manual `Debug` impls so `{:?}` on config/DSN types never prints a password.
fn redact(s: &str) -> &'static str {
    if s.is_empty() { "<unset>" } else { "<redacted>" }
}

/// A raw DSN string may still carry `user:<password>@host` (kept as-is when it has extra
/// parameters, see [`ConnectionConfig::dsn`]); mask just that span for `Debug`, same as the
/// form's DSN field does on screen ([`dsn::secret_span`]).
fn redact_dsn(text: &str) -> String {
    match dsn::secret_span(text) {
        Some((start, end)) => format!("{}{}{}", &text[..start], "<redacted>", &text[end..]),
        None => text.to_string(),
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Stable id. A profile read without one gets a new id when the config is loaded, and the
    /// launch-time migration writes it to the file.
    #[serde(default = "ProfileId::unset")]
    pub id: ProfileId,
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_driver")]
    pub driver: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    /// Legacy plaintext password from the file (moved to the keychain at launch).
    /// At runtime a resolved password is put here only on the copy handed to the driver.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub database: String,
    /// The TLS mode as the driver spells it ([`TlsMode::name`]); empty: none set (see
    /// [`TlsMode::default_for`]). Read with [`Self::tls`].
    #[serde(default)]
    pub sslmode: String,
    /// The CA file the server's certificate chain is checked against (`sslrootcert`).
    #[serde(default)]
    pub sslrootcert: Option<String>,
    /// The client certificate file (`sslcert`), with its key.
    #[serde(default)]
    pub sslcert: Option<String>,
    /// The client certificate's private key file (`sslkey`).
    #[serde(default)]
    pub sslkey: Option<String>,
    /// A DSN that could not be turned into fields (non-URL form or extra parameters). URL
    /// DSNs with only the TLS parameters are converted to fields when the file is loaded.
    #[serde(default)]
    pub dsn: Option<String>,
    /// Folder path (`work/prod`, validated by the config loader); `None` is the top level.
    #[serde(default)]
    pub folder: Option<String>,
    /// `red` … `gray` or `#rrggbb` (validated by the config loader); `None`: automatic.
    #[serde(default)]
    pub color: Option<String>,
    /// Name of a curated icon (the TUI's table); `None` or an unknown name: the driver's icon.
    #[serde(default)]
    pub icon: Option<String>,
    /// The environment's name (`[policy.<name>]`, see [`crate::policy`]); `None`: the default
    /// rules. A name no environment has is an error of the profile: it does not connect.
    #[serde(default)]
    pub policy: Option<String>,
    /// Where the password comes from; `None` is the keychain. See [`Self::source`].
    #[serde(default)]
    pub password_source: Option<SourceKind>,
    /// The command of `password_source = "command"`.
    #[serde(default)]
    pub password_command: Option<String>,
    /// The environment variable of `password_source = "env"`.
    #[serde(default)]
    pub password_env: Option<String>,
    /// Keep prepared statements on the server between transactions (the "server-side statement
    /// cache", on by default; written to the file only when off). Off for a server behind a
    /// pooler in transaction mode that does not support them (PgBouncer before 1.22, or with
    /// `max_prepared_statements = 0`); the driver also turns it off for a connection by itself
    /// when the server keeps losing them.
    #[serde(default = "yes")]
    pub statement_cache: bool,
    /// MySQL: the server's RSA public key, a PEM file (`server_public_key_file`; `None`: none).
    /// A `caching_sha2_password` login that sends the password over an unencrypted connection
    /// encrypts it with this key and never asks the server for one.
    #[serde(default)]
    pub server_public_key_file: Option<String>,
    /// MySQL: a direct connection to a server that is not on this machine may ask the server
    /// for its public key (`allow_public_key_retrieval`, off by default; written only when on).
    /// The key comes over the same unencrypted connection, so whoever sits in between could
    /// send theirs and read the password; off, such a login is refused. A loopback server
    /// and a server reached through an SSH tunnel are asked either way.
    #[serde(default)]
    pub allow_public_key_retrieval: bool,
    /// The SSH tunnel of this profile only (`[connections.ssh]`); `None`: never set up. Kept
    /// when turned off (see [`Self::inline_ssh`]).
    #[serde(default)]
    pub ssh: Option<ssh::SshSettings>,
    /// The tunnel preset the profile connects through (`tunnel = "<name>"`, see
    /// [`tunnel::route`]).
    #[serde(default)]
    pub tunnel: Option<String>,
    /// The Startup SQL preset its query sessions start with (`startup = "<name>"`, `"none"`, or
    /// no key: its environment's; see [`session::plan`]).
    #[serde(default, rename = "startup")]
    pub session: session::SessionRef,
    /// Its own statements its query sessions start with, after the preset's (`startup_sql`).
    #[serde(default, rename = "startup_sql")]
    pub session_sql: Vec<String>,
    /// `session`, the 0.31 name of `startup`, as the file has it: the loader moves it to
    /// [`Self::session`] (`None` once loaded).
    #[serde(default, rename = "session")]
    pub old_session: Option<session::SessionRef>,
    /// `session_sql`, the 0.31 name of `startup_sql`, the same way.
    #[serde(default, rename = "session_sql")]
    pub old_session_sql: Option<Vec<String>>,
    /// Name of this profile's table in the file, used to update it in place on save when the
    /// table has no `id` yet (the first save after the migration).
    #[serde(skip)]
    pub origin: Option<String>,
}

/// Manual impl (instead of `#[derive(Debug)]`) so `{:?}` never prints the plaintext password.
impl fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("driver", &self.driver)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &redact(&self.password))
            .field("database", &self.database)
            .field("sslmode", &self.sslmode)
            .field("sslrootcert", &self.sslrootcert)
            .field("sslcert", &self.sslcert)
            .field("sslkey", &self.sslkey)
            .field("dsn", &self.dsn.as_deref().map(redact_dsn))
            .field("folder", &self.folder)
            .field("color", &self.color)
            .field("icon", &self.icon)
            .field("policy", &self.policy)
            .field("password_source", &self.source())
            .field("statement_cache", &self.statement_cache)
            .field("server_public_key_file", &self.server_public_key_file)
            .field("allow_public_key_retrieval", &self.allow_public_key_retrieval)
            .field("ssh", &self.ssh)
            .field("tunnel", &self.tunnel)
            .field("session", &self.session)
            .field("session_sql", &self.session_sql)
            .field("origin", &self.origin)
            .finish()
    }
}

fn default_name() -> String {
    "local-pg".into()
}
fn default_driver() -> String {
    "postgres".into()
}
fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    5432
}
fn yes() -> bool {
    true
}

/// An unnamed PostgreSQL profile on `127.0.0.1:5432` with a new id.
impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            id: ProfileId::new(),
            name: String::new(),
            driver: default_driver(),
            host: default_host(),
            port: default_port(),
            user: String::new(),
            password: String::new(),
            database: String::new(),
            sslmode: String::new(),
            sslrootcert: None,
            sslcert: None,
            sslkey: None,
            dsn: None,
            folder: None,
            color: None,
            icon: None,
            policy: None,
            password_source: None,
            password_command: None,
            password_env: None,
            statement_cache: true,
            server_public_key_file: None,
            allow_public_key_retrieval: false,
            ssh: None,
            tunnel: None,
            session: session::SessionRef::Inherit,
            session_sql: Vec::new(),
            old_session: None,
            old_session_sql: None,
            origin: None,
        }
    }
}

impl ConnectionConfig {
    /// The development test database (`dev/`). A test helper only: the app no longer adds it
    /// when the config has no profiles (zero profiles is a real state).
    #[cfg(any(test, feature = "test-util"))]
    pub fn test_db() -> Self {
        Self {
            name: "local-pg".into(),
            port: 55432,
            user: "datarig".into(),
            password: "datarig".into(),
            database: "datarig".into(),
            sslmode: "disable".into(),
            ..Self::default()
        }
    }

    /// Where the password comes from. A `command` or `env` source without its setting has an
    /// empty one (the config loader rejects that; the form asks for it).
    pub fn source(&self) -> PasswordSource {
        match self.password_source.unwrap_or(SourceKind::Keychain) {
            SourceKind::Keychain => PasswordSource::Keychain,
            SourceKind::File => PasswordSource::File,
            SourceKind::Command => PasswordSource::Command(self.password_command.clone().unwrap_or_default()),
            SourceKind::Env => PasswordSource::Env(self.password_env.clone().unwrap_or_default()),
            SourceKind::Prompt => PasswordSource::Prompt,
        }
    }

    /// The profile's own SSH tunnel, when it is on (a preset it names is [`tunnel::route`]'s).
    pub fn inline_ssh(&self) -> Option<&ssh::SshSettings> {
        self.ssh.as_ref().filter(|s| s.enabled)
    }

    /// Set the source (the keychain, the default, is written as no key at all).
    pub fn set_source(&mut self, source: PasswordSource) {
        self.password_source = Some(source.kind()).filter(|k| *k != SourceKind::Keychain);
        self.password_command = None;
        self.password_env = None;
        match source {
            PasswordSource::Command(c) => self.password_command = Some(c),
            PasswordSource::Env(n) => self.password_env = Some(n),
            _ => {}
        }
    }

    /// The color to show: the one set, else the automatic color of the name.
    pub fn display_color(&self) -> color::ProfileColor {
        self.color
            .as_deref()
            .and_then(color::ProfileColor::parse)
            .unwrap_or_else(|| color::ProfileColor::auto(&self.name))
    }

    /// The folder, if the profile is in one.
    pub fn folder_path(&self) -> Option<folder::FolderPath> {
        self.folder.as_deref().and_then(|f| folder::FolderPath::parse(f).ok())
    }

    /// Password-free DSN of the fields (what the profile list and form show).
    pub fn display_dsn(&self) -> String {
        if let Some(d) = &self.dsn {
            return match dsn::parse(d) {
                Ok(mut p) => {
                    p.password = None;
                    dsn::format(&p)
                }
                Err(_) => d.clone(),
            };
        }
        let scheme = dsn::Scheme::of_driver(&self.driver);
        dsn::format(&dsn::Dsn {
            scheme,
            user: self.user.clone(),
            password: None,
            host: self.host.clone(),
            port: Some(self.port),
            database: self.database.clone(),
            // MySQL URLs take no parameters: their TLS settings are fields only.
            params: match scheme {
                dsn::Scheme::Postgres => self.tls_params(),
                dsn::Scheme::MySql => Vec::new(),
            },
        })
    }

    /// The PostgreSQL URL parameters of the TLS fields: `sslmode` (the default's name when none
    /// is set), then the files that are set.
    pub fn tls_params(&self) -> Vec<(String, String)> {
        let mode = match self.sslmode.trim() {
            "" => TlsMode::default_for(dsn::Scheme::Postgres, false).name(dsn::Scheme::Postgres).to_string(),
            m => m.to_string(),
        };
        let mut params = vec![("sslmode".to_string(), mode)];
        for (file, value) in
            [(TlsFile::Ca, &self.sslrootcert), (TlsFile::Cert, &self.sslcert), (TlsFile::Key, &self.sslkey)]
        {
            if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                params.push((file.key().to_string(), v.to_string()));
            }
        }
        params
    }

    /// `(host:port, database, sslmode)` for lists of profiles. A DSN that is not a
    /// URL is shown whole as the address.
    pub fn endpoint(&self) -> (String, String, String) {
        let host_port = |h: &str, p: Option<u16>| {
            let h = if h.contains(':') { format!("[{h}]") } else { h.to_string() };
            p.map_or(h.clone(), |p| format!("{h}:{p}"))
        };
        match &self.dsn {
            Some(d) => match dsn::parse(d) {
                Ok(p) => (host_port(&p.host, p.port), p.database.clone(), p.param("sslmode").unwrap_or("").to_string()),
                Err(_) => (d.clone(), String::new(), String::new()),
            },
            None => (host_port(&self.host, Some(self.port)), self.database.clone(), self.sslmode.clone()),
        }
    }

    /// Convert a URL `dsn` into fields when nothing would be lost: a URL of the profile's
    /// driver with no parameters but PostgreSQL's TLS ones (a known `sslmode`, `sslrootcert`,
    /// `sslcert`, `sslkey`, each once), whose file fields the profile does not set already.
    pub(crate) fn normalize(&mut self) {
        let Some(text) = &self.dsn else { return };
        let Ok(d) = dsn::parse(text) else { return };
        if d.scheme != dsn::Scheme::of_driver(&self.driver) {
            return;
        }
        let tls = |k: &str, v: &str| {
            d.scheme == dsn::Scheme::Postgres
                && d.params.iter().filter(|(other, _)| other == k).count() == 1
                && match k {
                    "sslmode" => SSL_MODES.contains(&v),
                    "sslrootcert" => self.sslrootcert.is_none(),
                    "sslcert" => self.sslcert.is_none(),
                    "sslkey" => self.sslkey.is_none(),
                    _ => false,
                }
        };
        if d.params.iter().any(|(k, v)| !tls(k, v)) {
            return;
        }
        self.host = if d.host.is_empty() { default_host() } else { d.host.clone() };
        self.port = d.port.unwrap_or(d.scheme.default_port());
        self.user = d.user.clone();
        self.database = d.database.clone();
        if let Some(m) = d.param("sslmode") {
            self.sslmode = m.to_string();
        }
        self.sslrootcert = d.param("sslrootcert").map(str::to_string).or(self.sslrootcert.take());
        self.sslcert = d.param("sslcert").map(str::to_string).or(self.sslcert.take());
        self.sslkey = d.param("sslkey").map(str::to_string).or(self.sslkey.take());
        if let Some(p) = d.password {
            self.password = p;
        }
        self.dsn = None;
    }

    /// The profile's TLS settings, checked ([`TlsSettings::new`]): from its fields, and for a
    /// PostgreSQL profile kept as a DSN, from the DSN's own parameters (`sslmode`,
    /// `sslrootcert`, `sslcert`, `sslkey`, in a URL or in `key=value` form), each over the
    /// field of the same name.
    pub fn tls(&self) -> Result<TlsSettings, TlsProblem> {
        let scheme = dsn::Scheme::of_driver(&self.driver);
        let pinned = self.server_public_key_file.as_deref().is_some_and(|k| !k.trim().is_empty());
        let mut mode = self.sslmode.clone();
        let mut files = [self.sslrootcert.clone(), self.sslcert.clone(), self.sslkey.clone()];
        if let (dsn::Scheme::Postgres, Some(text)) = (scheme, &self.dsn) {
            // A URL's parameters, also of one with several hosts; a `key=value` string's pairs.
            let params = match dsn::is_url(text) {
                true => dsn::parse(text).map(|d| d.params).ok().or_else(|| dsn::url_params(text)),
                false => dsn::conninfo(text).map(|c| c.into_iter().map(|p| (p.key, p.value)).collect()),
            };
            let params = params.ok_or(TlsProblem::DsnUnreadable)?;
            for (k, v) in params {
                match k.as_str() {
                    "sslmode" => mode = v,
                    "sslrootcert" => files[0] = Some(v),
                    "sslcert" => files[1] = Some(v),
                    "sslkey" => files[2] = Some(v),
                    _ => {}
                }
            }
        }
        let [ca, cert, key] = &files;
        TlsSettings::new(scheme, &mode, [ca.as_deref(), cert.as_deref(), key.as_deref()], pinned)
    }

    /// Whether the profile's connection goes through the network on its own: directly (no SSH
    /// tunnel) to a server that is not on this machine. Unless it is encrypted (TLS), the
    /// password exchange and every result can then be read (or changed) on the way.
    pub fn exposed(&self) -> bool {
        if self.tunnel.is_some() || self.inline_ssh().is_some() {
            return false;
        }
        // The driver's hosts: a URL's (none: `localhost`), a `key=value` string's `host` and
        // `hostaddr` (none: the server's socket on this machine), else the profile's.
        let hosts = match self.dsn.as_deref() {
            Some(text) if dsn::is_url(text) => match dsn::parse(text) {
                Ok(d) if d.host.is_empty() => vec!["localhost".to_string()],
                Ok(d) => vec![d.host],
                Err(_) => return true,
            },
            Some(text) => match dsn::conninfo(text) {
                Some(pairs) => pairs
                    .into_iter()
                    .filter(|p| p.key == "host" || p.key == "hostaddr")
                    .flat_map(|p| p.value.split(',').map(str::to_string).collect::<Vec<_>>())
                    .collect(),
                None => return true,
            },
            None => vec![self.host.clone()],
        };
        hosts.iter().any(|h| !is_local(h.trim()))
    }

    /// Whether the profile's connection is sure to go through the network unencrypted:
    /// [`Self::exposed`], and its TLS mode is off (`disable`). With `prefer` it may still
    /// be; the connection says what it got ([`crate::tls::Encryption`]).
    pub fn unencrypted(&self) -> bool {
        self.exposed() && self.tls().is_ok_and(|t| t.mode == TlsMode::Disable)
    }
}

/// Whether a connection to `host` stays on this machine: a loopback host ([`is_loopback`]), a
/// PostgreSQL socket directory (`/run/postgresql`) or an abstract socket (`@name`), or no host
/// at all (the server's socket).
fn is_local(host: &str) -> bool {
    host.is_empty() || host.starts_with('/') || host.starts_with('@') || is_loopback(host)
}

/// Whether `host` names this machine: `localhost` or a loopback address (`127.0.0.0/8`,
/// `::1`, also in brackets or IPv4-mapped).
pub fn is_loopback(host: &str) -> bool {
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

#[cfg(test)]
mod tests;
