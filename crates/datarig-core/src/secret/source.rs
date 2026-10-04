//! Per-profile password sources. A profile says where its
//! password comes from; the config file keeps only that choice, never the password:
//!
//! | source | config | where the password is |
//! |---|---|---|
//! | `keychain` (default) | nothing, or `password_source = "keychain"` | the OS keychain, account `profile:<uuid>` |
//! | `file` | `password_source = "file"` | `secrets.toml` next to the config file ([`super::FileStore`]) |
//! | `command` | `password_source = "command"`, `password_command = "…"` | the standard output of the command ([`super::command`]) |
//! | `env` | `password_source = "env"`, `password_env = "NAME"` | the environment variable, read at connect time |
//! | `prompt` | `password_source = "prompt"` | asked on every connect, never stored |
//!
//! [`lookup`] resolves a source (blocking: the keychain may wait for the user, a command may
//! run for up to its timeout). [`switch`] moves a stored password when a profile changes its
//! source: the new copy is written and read back, the config is saved, and only then is the
//! old copy removed.

use super::command::{self, CommandError};
use super::file::FileError;
use super::{SecretStore, StoreError, Stores, Unavailable, store_verified};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

/// The kind of a password source (`password_source` in the config).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Keychain,
    File,
    Command,
    Env,
    Prompt,
}

impl SourceKind {
    /// Every kind, in the order the profile form offers them.
    pub const ALL: [SourceKind; 5] =
        [SourceKind::Keychain, SourceKind::File, SourceKind::Command, SourceKind::Env, SourceKind::Prompt];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Keychain => "keychain",
            SourceKind::File => "file",
            SourceKind::Command => "command",
            SourceKind::Env => "env",
            SourceKind::Prompt => "prompt",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// datarig stores the password itself (keychain, secrets file).
    pub fn stores_secret(self) -> bool {
        matches!(self, SourceKind::Keychain | SourceKind::File)
    }
}

impl<'de> Deserialize<'de> for SourceKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        SourceKind::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "password_source = {s:?}: expected keychain, file, command, env or prompt"
            ))
        })
    }
}

/// `[secrets] default_source`: the source a new profile starts with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DefaultSource {
    /// The keychain when it works, else `prompt`.
    #[default]
    Auto,
    Kind(SourceKind),
}

impl DefaultSource {
    /// `auto` first, then every kind.
    pub const ALL: [DefaultSource; 6] = [
        DefaultSource::Auto,
        DefaultSource::Kind(SourceKind::Keychain),
        DefaultSource::Kind(SourceKind::File),
        DefaultSource::Kind(SourceKind::Command),
        DefaultSource::Kind(SourceKind::Env),
        DefaultSource::Kind(SourceKind::Prompt),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DefaultSource::Auto => "auto",
            DefaultSource::Kind(k) => k.as_str(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.trim().eq_ignore_ascii_case("auto") {
            return Some(DefaultSource::Auto);
        }
        SourceKind::parse(s).map(DefaultSource::Kind)
    }

    /// The kind a new profile gets. The keychain is never preselected when it does not work.
    pub fn resolve(self, keychain_available: bool) -> SourceKind {
        match self {
            DefaultSource::Auto | DefaultSource::Kind(SourceKind::Keychain) if !keychain_available => {
                SourceKind::Prompt
            }
            DefaultSource::Auto => SourceKind::Keychain,
            DefaultSource::Kind(k) => k,
        }
    }
}

/// A profile's password source with its setting.
#[derive(Clone, PartialEq, Eq)]
pub enum PasswordSource {
    Keychain,
    File,
    /// The command line to run (split with shell rules, run without a shell).
    Command(String),
    /// The name of the environment variable.
    Env(String),
    Prompt,
}

/// The command may carry a secret of its own (`echo …`), so `Debug` never prints it.
impl std::fmt::Debug for PasswordSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordSource::Command(c) => write!(f, "Command({})", if c.is_empty() { "<unset>" } else { "<set>" }),
            PasswordSource::Env(n) => write!(f, "Env({n:?})"),
            other => write!(f, "{}", other.kind().as_str()),
        }
    }
}

impl PasswordSource {
    pub fn kind(&self) -> SourceKind {
        match self {
            PasswordSource::Keychain => SourceKind::Keychain,
            PasswordSource::File => SourceKind::File,
            PasswordSource::Command(_) => SourceKind::Command,
            PasswordSource::Env(_) => SourceKind::Env,
            PasswordSource::Prompt => SourceKind::Prompt,
        }
    }
}

/// A valid environment variable name: `[A-Za-z_][A-Za-z0-9_]*`.
pub fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Why a source could not give a password. Every variant names its source, so the message
/// can say which one failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The OS keychain failed.
    Keychain(crate::fault::Fault),
    File(FileError),
    Command(CommandError),
    /// The environment variable is not set (or empty).
    EnvMissing(String),
}

impl SourceError {
    pub fn kind(&self) -> SourceKind {
        match self {
            SourceError::Keychain(_) => SourceKind::Keychain,
            SourceError::File(_) => SourceKind::File,
            SourceError::Command(_) => SourceKind::Command,
            SourceError::EnvMissing(_) => SourceKind::Env,
        }
    }
}

/// What a source gave.
#[derive(Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(String),
    /// A storing source has no password for the profile (yet).
    Missing,
    /// `prompt`: ask the user.
    Prompt,
}

impl std::fmt::Debug for Lookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Lookup::Found(_) => "Found(<redacted>)",
            Lookup::Missing => "Missing",
            Lookup::Prompt => "Prompt",
        })
    }
}

/// Resolve `source` for `account` (`profile:<uuid>`). `env` reads environment variables (the
/// process environment in the app, a table in tests); `timeout` limits a command. Blocking.
pub fn lookup(
    source: &PasswordSource,
    account: &str,
    stores: &Stores,
    env: &dyn Fn(&str) -> Option<String>,
    timeout: Duration,
) -> Result<Lookup, SourceError> {
    let found = |v: Option<String>| v.map_or(Lookup::Missing, Lookup::Found);
    match source {
        PasswordSource::Keychain => {
            stores.keychain.get(account).map(found).map_err(|Unavailable(e)| SourceError::Keychain(e))
        }
        PasswordSource::File => {
            stores.check_file().map_err(SourceError::File)?;
            stores.file.get(account).map(found).map_err(|Unavailable(e)| SourceError::File(FileError::Io(e)))
        }
        PasswordSource::Command(cmd) => command::run(cmd, timeout).map(Lookup::Found).map_err(SourceError::Command),
        PasswordSource::Env(name) => match env(name) {
            Some(v) if !v.is_empty() => Ok(Lookup::Found(v)),
            _ => Err(SourceError::EnvMissing(name.clone())),
        },
        PasswordSource::Prompt => Ok(Lookup::Prompt),
    }
}

/// The store behind a storing source.
fn store_of(stores: &Stores, kind: SourceKind) -> Option<&dyn SecretStore> {
    match kind {
        SourceKind::Keychain => Some(stores.keychain.as_ref()),
        SourceKind::File => Some(stores.file.as_ref()),
        _ => None,
    }
}

fn store_failure(kind: SourceKind, e: crate::fault::Fault) -> SourceError {
    match kind {
        SourceKind::File => SourceError::File(FileError::Io(e)),
        _ => SourceError::Keychain(e),
    }
}

/// Why a source change stopped. Nothing was removed in any of these cases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchError {
    /// The password could not be read from the old store (and none was typed).
    Read(SourceError),
    /// Writing or reading back the new copy failed; the old copy is untouched.
    Write(SourceError),
    /// The new store gave back something else than what was written.
    Mismatch(SourceKind),
    /// Saving the config failed; both copies (if any) are kept.
    Config(crate::fault::Fault),
}

/// What a finished source change did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switched {
    /// A password was written to the new store (and read back).
    pub copied: bool,
    /// The old store's copy was removed.
    pub removed: bool,
    /// Removing the old copy failed (the change itself is saved; the old copy stays).
    pub remove_failed: Option<SourceError>,
}

/// Change a profile's source from `from` to `to` without ever losing its password:
///
/// 1. When `to` stores passwords: the password is `typed` (the form's field) or, when `from`
///    stores one, the old copy. It is written to the new store and read back ([`store_verified`]).
/// 2. `save_config` records the new source.
/// 3. Only then is the old copy removed (when `from` stores passwords in another store).
///
/// Any failure in 1–2 stops before anything is removed. Steps 1 and 3 are also
/// [`switch_copy`] and [`switch_remove`], for a caller that runs them off its thread (the
/// keychain may take its time) and saves the config in between on its own.
pub fn switch(
    stores: &Stores,
    from: SourceKind,
    to: SourceKind,
    account: &str,
    typed: Option<&str>,
    save_config: impl FnOnce() -> Result<(), crate::fault::Fault>,
) -> Result<Switched, SwitchError> {
    let copied = switch_copy(stores, from, to, account, typed)?;
    save_config().map_err(SwitchError::Config)?;
    let mut out = switch_remove(stores, from, to, account);
    out.copied = copied;
    Ok(out)
}

/// Step 1 of [`switch`]: the password written to the new store and read back. `Ok(true)` when
/// one was copied (there may be none: nothing typed and nothing stored).
pub fn switch_copy(
    stores: &Stores,
    from: SourceKind,
    to: SourceKind,
    account: &str,
    typed: Option<&str>,
) -> Result<bool, SwitchError> {
    let Some(new) = store_of(stores, to) else { return Ok(false) };
    let secret = match typed.filter(|t| !t.is_empty()) {
        Some(t) => Some(t.to_string()),
        None if from == to => None,
        None => match store_of(stores, from) {
            Some(old) => {
                if from == SourceKind::File {
                    stores.check_file().map_err(|e| SwitchError::Read(SourceError::File(e)))?;
                }
                old.get(account).map_err(|Unavailable(e)| SwitchError::Read(store_failure(from, e)))?
            }
            None => None,
        },
    };
    let Some(secret) = secret else { return Ok(false) };
    if to == SourceKind::File {
        stores.check_file().map_err(|e| SwitchError::Write(SourceError::File(e)))?;
    }
    match store_verified(new, account, &secret) {
        Ok(()) => Ok(true),
        Err(StoreError::Mismatch) => Err(SwitchError::Mismatch(to)),
        Err(StoreError::Unavailable(e)) => Err(SwitchError::Write(store_failure(to, e))),
    }
}

/// Step 3 of [`switch`], once the config says `to`: the old store's copy removed (when `from`
/// stores passwords in another store). `copied` is left `false`.
pub fn switch_remove(stores: &Stores, from: SourceKind, to: SourceKind, account: &str) -> Switched {
    let mut out = Switched::default();
    if from != to
        && let Some(old) = store_of(stores, from)
    {
        match old.delete(account) {
            Ok(removed) => out.removed = removed,
            Err(Unavailable(e)) => out.remove_failed = Some(store_failure(from, e)),
        }
    }
    out
}

/// Step 1 of moving a stored secret from account `from` in the store of `from_kind` (`None`: a
/// source that stores none) to account `to` in the store of `to_kind` (a profile's own tunnel
/// secret becoming a tunnel preset's, maybe in another store): the secret (`typed`, else the old
/// copy) written under `to` and read back. `Ok(true)` when one was written (there may be none:
/// nothing typed and nothing stored). The caller then saves the config, and only then removes
/// the old copy ([`rekey_remove`]); any failure here leaves the old copy where it is.
pub fn rekey_copy(
    stores: &Stores,
    from_kind: Option<SourceKind>,
    from: &str,
    to_kind: SourceKind,
    to: &str,
    typed: Option<&str>,
) -> Result<bool, SwitchError> {
    let Some(new) = store_of(stores, to_kind) else { return Ok(false) };
    let secret = match typed.filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => match from_kind.and_then(|k| store_of(stores, k).map(|s| (k, s))) {
            Some((kind, old)) => {
                if kind == SourceKind::File {
                    stores.check_file().map_err(|e| SwitchError::Read(SourceError::File(e)))?;
                }
                match old.get(from).map_err(|Unavailable(e)| SwitchError::Read(store_failure(kind, e)))? {
                    Some(s) => s,
                    None => return Ok(false),
                }
            }
            None => return Ok(false),
        },
    };
    if to_kind == SourceKind::File {
        stores.check_file().map_err(|e| SwitchError::Write(SourceError::File(e)))?;
    }
    match store_verified(new, to, &secret) {
        Ok(()) => Ok(true),
        Err(StoreError::Mismatch) => Err(SwitchError::Mismatch(to_kind)),
        Err(StoreError::Unavailable(e)) => Err(SwitchError::Write(store_failure(to_kind, e))),
    }
}

/// Step 3 of a move ([`rekey_copy`]), once the config no longer uses `from`: its copy removed.
pub fn rekey_remove(stores: &Stores, kind: SourceKind, from: &str) -> Switched {
    let mut out = Switched::default();
    if let Some(store) = store_of(stores, kind) {
        match store.delete(from) {
            Ok(removed) => out.removed = removed,
            Err(Unavailable(e)) => out.remove_failed = Some(store_failure(kind, e)),
        }
    }
    out
}

#[cfg(test)]
mod tests;
