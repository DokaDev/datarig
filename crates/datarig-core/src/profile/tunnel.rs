//! Tunnel presets: named SSH tunnels several profiles can use, kept next to the profiles in the
//! config file as `[tunnels.<name>]` tables:
//!
//! ```toml
//! [tunnels.office]
//! id = "…"                        # written by datarig; keeps the secret when the name changes
//! host = "bastion.example.com"
//! port = 22
//! user = "ec2-user"
//! auth = "key"                    # key | password | agent | keyboard-interactive
//! key_file = "~/.ssh/office.pem"  # auth = "key"
//! secret_source = "keychain"      # the key's passphrase or the password (as password_source)
//! keepalive = 15                  # seconds; 0: none
//! timeout = 10                    # seconds per step
//!
//! [[connections]]
//! name = "orders"
//! tunnel = "office"
//! ```
//!
//! A profile reaches its database directly, through a preset (`tunnel = "<name>"`) or through
//! its own `[connections.ssh]` table ([`route`]); a preset that is not there, or both at once,
//! is an error of that profile, never a direct connection. A preset's secret is kept once, under
//! its own account ([`TunnelId::account`]), whatever its name.

use super::ConnectionConfig;
use super::ssh::{SshAuth, SshSettings};
use crate::secret::SourceKind;
use serde::{Deserialize, Deserializer};
use std::fmt;
use uuid::Uuid;

/// A preset's stable internal id: a UUID v4 that never changes, unlike the name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TunnelId(Uuid);

impl TunnelId {
    /// A new random id.
    pub fn new() -> Self {
        TunnelId(Uuid::new_v4())
    }

    /// Placeholder of a preset read without an `id` (the config loader replaces it).
    pub(crate) fn unset() -> Self {
        TunnelId(Uuid::nil())
    }

    pub(crate) fn is_unset(&self) -> bool {
        self.0.is_nil()
    }

    /// A UUID in any standard text form; the nil UUID is not an id.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().filter(|u| !u.is_nil()).map(TunnelId)
    }

    /// The keychain (and secrets file) account of the preset's secret.
    pub fn account(&self) -> String {
        format!("tunnel:{self}")
    }
}

impl Default for TunnelId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TunnelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl fmt::Debug for TunnelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TunnelId({self})")
    }
}

impl<'de> Deserialize<'de> for TunnelId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        TunnelId::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("id = {s:?} is not a UUID")))
    }
}

/// A named tunnel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelPreset {
    pub id: TunnelId,
    /// The table's key in the file, and what profiles name (`tunnel = "<name>"`).
    pub name: String,
    /// Always on (`enabled` is not a key of a preset).
    pub settings: SshSettings,
    /// The name its table has in the file (a rename moves that table, comments and all).
    pub origin: Option<String>,
}

impl TunnelPreset {
    /// A new preset `name` with `settings` (turned on) and a new id.
    pub fn new(name: &str, settings: SshSettings) -> Self {
        TunnelPreset {
            id: TunnelId::new(),
            name: name.to_string(),
            settings: SshSettings { enabled: true, ..settings },
            origin: None,
        }
    }
}

/// `[tunnels.<name>]` as the file has it: the keys of `[connections.ssh]` without `enabled`, and
/// the preset's `id`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TunnelSection {
    #[serde(default = "TunnelId::unset")]
    pub id: TunnelId,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub auth: SshAuth,
    #[serde(default)]
    pub key_file: Option<String>,
    #[serde(default)]
    pub secret_source: Option<SourceKind>,
    #[serde(default)]
    pub secret_command: Option<String>,
    #[serde(default)]
    pub secret_env: Option<String>,
    #[serde(default)]
    pub keepalive: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

fn default_port() -> u16 {
    22
}

impl TunnelSection {
    pub(crate) fn into_preset(self, name: &str) -> TunnelPreset {
        TunnelPreset {
            id: self.id,
            name: name.to_string(),
            settings: SshSettings {
                enabled: true,
                host: self.host,
                port: self.port,
                user: self.user,
                auth: self.auth,
                key_file: self.key_file,
                secret_source: self.secret_source,
                secret_command: self.secret_command,
                secret_env: self.secret_env,
                keepalive: self.keepalive,
                timeout: self.timeout,
            },
            origin: Some(name.to_string()),
        }
    }
}

/// The longest name a preset may have (characters).
pub const NAME_MAX: usize = 64;

/// Why a preset's name cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    /// Blanks before or after it.
    Spaces,
    /// A control character (a tab, a line break, …).
    Control,
    /// Longer than [`NAME_MAX`].
    TooLong,
}

/// What keeps `name` from being a preset's name (`None`: it can be one).
pub fn name_problem(name: &str) -> Option<NameProblem> {
    if name.is_empty() {
        Some(NameProblem::Empty)
    } else if name.trim() != name {
        Some(NameProblem::Spaces)
    } else if name.chars().any(char::is_control) {
        Some(NameProblem::Control)
    } else if name.chars().count() > NAME_MAX {
        Some(NameProblem::TooLong)
    } else {
        None
    }
}

/// Two names a person would take for the same preset (they differ only in case).
pub fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// How a profile reaches its database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route<'a> {
    Direct,
    /// Its own `[connections.ssh]` table (turned on).
    Inline(&'a SshSettings),
    /// The preset it names.
    Preset(&'a TunnelPreset),
}

impl<'a> Route<'a> {
    /// The tunnel's settings (`None`: direct).
    pub fn settings(self) -> Option<&'a SshSettings> {
        match self {
            Route::Direct => None,
            Route::Inline(s) => Some(s),
            Route::Preset(p) => Some(&p.settings),
        }
    }
}

/// Why a profile cannot connect: its tunnel setting is wrong. It never connects directly then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteError {
    /// `tunnel = "<tunnel>"` and an `[connections.ssh]` table that is on: which one is meant is
    /// not guessed.
    Both { tunnel: String },
    /// No preset has this name.
    NotFound(String),
}

/// How profile `c` reaches its database with the presets `presets`.
pub fn route<'a>(c: &'a ConnectionConfig, presets: &'a [TunnelPreset]) -> Result<Route<'a>, RouteError> {
    match (c.tunnel.as_deref(), c.inline_ssh()) {
        (Some(name), Some(_)) => Err(RouteError::Both { tunnel: name.to_string() }),
        (Some(name), None) => match presets.iter().find(|p| p.name == name) {
            Some(p) => Ok(Route::Preset(p)),
            None => Err(RouteError::NotFound(name.to_string())),
        },
        (None, Some(s)) => Ok(Route::Inline(s)),
        (None, None) => Ok(Route::Direct),
    }
}

#[cfg(test)]
mod tests;
