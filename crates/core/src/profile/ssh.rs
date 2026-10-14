//! A profile's SSH tunnel: one bastion, entered in the
//! profile (never read from `~/.ssh/config`), in the file as the profile's `[connections.ssh]`
//! table:
//!
//! ```toml
//! [connections.ssh]
//! enabled = true
//! host = "bastion.example.com"
//! port = 22
//! user = "ec2-user"
//! auth = "key"                    # key | password | agent | keyboard-interactive
//! key_file = "~/.ssh/prod.pem"    # auth = "key"
//! secret_source = "keychain"      # the key's passphrase or the password (as password_source)
//! keepalive = 15                  # seconds; 0: none
//! timeout = 10                    # seconds per step
//! ```
//!
//! The table stays when the tunnel is turned off (`enabled = false`). Its secret is kept apart
//! from the database password ([`SshSettings::account`]).

use crate::secret::{PasswordSource, SourceKind};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

/// How the tunnel logs in to the bastion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SshAuth {
    /// A private key file (OpenSSH or PEM), maybe with a passphrase.
    #[default]
    Key,
    Password,
    /// The keys of the ssh-agent (`SSH_AUTH_SOCK`).
    Agent,
    /// The bastion's own questions (a one-time code), answered when connecting.
    KeyboardInteractive,
}

impl SshAuth {
    pub const ALL: [SshAuth; 4] = [SshAuth::Key, SshAuth::Password, SshAuth::Agent, SshAuth::KeyboardInteractive];

    pub fn as_str(self) -> &'static str {
        match self {
            SshAuth::Key => "key",
            SshAuth::Password => "password",
            SshAuth::Agent => "agent",
            SshAuth::KeyboardInteractive => "keyboard-interactive",
        }
    }

    /// It has a secret datarig may keep (the key's passphrase, the password).
    pub fn has_secret(self) -> bool {
        matches!(self, SshAuth::Key | SshAuth::Password)
    }
}

/// Seconds between keepalives unless the profile says otherwise.
pub const DEFAULT_KEEPALIVE: u64 = 15;
/// Seconds each step of opening the tunnel may take unless the profile says otherwise.
pub const DEFAULT_TIMEOUT: u64 = 10;

/// The profile's tunnel settings.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub auth: SshAuth,
    /// `~` is the home directory.
    #[serde(default)]
    pub key_file: Option<String>,
    /// Where the passphrase or password comes from; `None` is the keychain.
    #[serde(default)]
    pub secret_source: Option<SourceKind>,
    #[serde(default)]
    pub secret_command: Option<String>,
    #[serde(default)]
    pub secret_env: Option<String>,
    /// Seconds; `0`: no keepalive; `None`: [`DEFAULT_KEEPALIVE`].
    #[serde(default)]
    pub keepalive: Option<u64>,
    /// Seconds; `None`: [`DEFAULT_TIMEOUT`].
    #[serde(default)]
    pub timeout: Option<u64>,
}

fn default_port() -> u16 {
    22
}

/// Manual impl so `{:?}` never prints the secret's command (it may carry a secret of its own,
/// as `password_command` may).
impl std::fmt::Debug for SshSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSettings")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("auth", &self.auth)
            .field("key_file", &self.key_file)
            .field("secret_source", &self.source())
            .field("keepalive", &self.keepalive)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Default for SshSettings {
    fn default() -> Self {
        SshSettings {
            enabled: false,
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: SshAuth::Key,
            key_file: None,
            secret_source: None,
            secret_command: None,
            secret_env: None,
            keepalive: None,
            timeout: None,
        }
    }
}

/// Why tunnel settings cannot be used (the config loader and the form say which key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshProblem {
    /// `key` is required and empty.
    Missing(&'static str),
    /// `key` has a value that is not allowed (`port = 0`, …).
    Invalid(&'static str),
}

impl SshSettings {
    /// Where its secret comes from (as [`crate::profile::ConnectionConfig::source`]).
    pub fn source(&self) -> PasswordSource {
        match self.secret_source.unwrap_or(SourceKind::Keychain) {
            SourceKind::Keychain => PasswordSource::Keychain,
            SourceKind::File => PasswordSource::File,
            SourceKind::Command => PasswordSource::Command(self.secret_command.clone().unwrap_or_default()),
            SourceKind::Env => PasswordSource::Env(self.secret_env.clone().unwrap_or_default()),
            SourceKind::Prompt => PasswordSource::Prompt,
        }
    }

    /// Set the source (the keychain, the default, is written as no key at all).
    pub fn set_source(&mut self, source: PasswordSource) {
        self.secret_source = Some(source.kind()).filter(|k| *k != SourceKind::Keychain);
        self.secret_command = None;
        self.secret_env = None;
        match source {
            PasswordSource::Command(c) => self.secret_command = Some(c),
            PasswordSource::Env(n) => self.secret_env = Some(n),
            _ => {}
        }
    }

    /// The keychain (and secrets file) account of profile `id`'s tunnel secret.
    pub fn account(id: crate::profile::ProfileId) -> String {
        format!("{}:ssh", id.account())
    }

    /// The key file with `~` expanded (`home`: the home directory).
    pub fn key_path(&self, home: Option<&std::path::Path>) -> Option<PathBuf> {
        let f = self.key_file.as_deref().map(str::trim).filter(|f| !f.is_empty())?;
        match (f.strip_prefix("~/").or_else(|| f.strip_prefix("~\\")), home) {
            (Some(rest), Some(h)) => Some(h.join(rest)),
            _ if f == "~" => home.map(PathBuf::from),
            _ => Some(PathBuf::from(f)),
        }
    }

    pub fn keepalive(&self) -> Option<Duration> {
        match self.keepalive.unwrap_or(DEFAULT_KEEPALIVE) {
            0 => None,
            s => Some(Duration::from_secs(s)),
        }
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(DEFAULT_TIMEOUT).max(1))
    }

    /// What keeps an enabled tunnel from being used (a disabled one is never checked).
    pub fn problem(&self) -> Option<SshProblem> {
        if !self.enabled {
            return None;
        }
        if self.host.trim().is_empty() {
            return Some(SshProblem::Missing("host"));
        }
        if self.port == 0 {
            return Some(SshProblem::Invalid("port"));
        }
        if self.user.trim().is_empty() {
            return Some(SshProblem::Missing("user"));
        }
        if self.auth == SshAuth::Key && self.key_file.as_deref().is_none_or(|f| f.trim().is_empty()) {
            return Some(SshProblem::Missing("key_file"));
        }
        if self.auth.has_secret() {
            match self.secret_source {
                Some(SourceKind::Command) if self.secret_command.as_deref().is_none_or(|c| c.trim().is_empty()) => {
                    return Some(SshProblem::Missing("secret_command"));
                }
                Some(SourceKind::Env) => match self.secret_env.as_deref() {
                    None => return Some(SshProblem::Missing("secret_env")),
                    Some(n) if !crate::secret::source::valid_env_name(n) => {
                        return Some(SshProblem::Invalid("secret_env"));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
