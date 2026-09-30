//! The launch-time migration of the config file and the keychain. It runs when the config file was read without errors and
//! [`Config::needs_migration`]: a version 1 file, profiles without ids, or plaintext
//! passwords. Running it again gives the same result.
//!
//! 1. A version 1 file is copied to `config.toml.v1.bak` once (an existing backup is kept).
//! 2. Profiles without an `id` already got one in memory when the file was loaded.
//! 3. The file is saved with the ids first. If that fails, stop: the keychain is not touched,
//!    this run works with the ids in memory, and the caller warns.
//! 4. Keychain: one probe first. An unavailable store (headless Linux, WSL, …) makes the
//!    keychain steps not applicable: name-keyed entries could never have been created there.
//!    Otherwise, for each profile of a version 1 file, its name-keyed entry moves to
//!    `profile:<uuid>`: an existing `profile:<uuid>` entry is **never** overwritten (the old
//!    entry is removed only when it holds the same password; otherwise both stay and the
//!    caller tells the user once); a new one is written and read back, and only then is the
//!    old one removed.
//! 5. Plaintext passwords move to `profile:<uuid>` under the same rules (written, read back,
//!    only then dropped from the file; never over a different existing entry). The store is the
//!    profile's source: the keychain, or the secrets file for `password_source = "file"` (which
//!    does not depend on the keychain probe). `command`, `env` and `prompt` profiles never use a
//!    plaintext password; it is left alone.
//! 6. `last_used` was already resolved from a name to an id when the file was loaded.
//! 7. When every keychain step succeeded (or none applied), the file is saved again with
//!    `version = 2` and without the moved passwords. Otherwise `version` stays, so the next
//!    launch retries; the passwords that did move are dropped from the file already.
//!
//! Blocking (the OS may ask the user for permission), so the TUI runs it off its thread.

use crate::config::{self, CONFIG_VERSION, Config, IconsSetting, Profiles, Settings};
use crate::fault::{Fault, FaultKind, KeychainFault};
use crate::profile::ProfileId;
use crate::secret::{DefaultSource, SecretStore, SourceKind, StoreError, Unavailable, store_verified};
use std::path::PathBuf;

/// What the migration needs: the loaded config and the settings the app would save.
#[derive(Clone, Debug)]
pub struct Input {
    pub config: Config,
    pub language: String,
    pub icons: IconsSetting,
    pub theme: String,
    pub default_source: DefaultSource,
    pub prefs: config::Prefs,
}

impl Input {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            language: config.language.clone(),
            icons: config.icons,
            theme: config.theme.clone(),
            default_source: config.default_source,
            prefs: config.prefs,
        }
    }
}

/// How the keychain part went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Keychain {
    /// Every keychain step succeeded (or there was nothing to move).
    Done,
    /// No usable store: the keychain steps do not apply; plaintext passwords stay in the file.
    Unavailable(Fault),
    /// Some entries could not be moved; `version` stays and the next launch retries.
    Partial,
}

/// The outcome, for the app to apply and report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The version 1 backup that was written this time.
    pub backup: Option<PathBuf>,
    /// Saving the ids failed: nothing else was done.
    pub save_failed: Option<Fault>,
    pub keychain: Keychain,
    /// Name-keyed keychain entries moved to `profile:<uuid>`.
    pub rekeyed: u64,
    /// Profiles whose plaintext password moved to the keychain (and left the file).
    pub moved: Vec<ProfileId>,
    /// Profile names with an existing `profile:<uuid>` entry that differs from the old entry or
    /// the plaintext: both were kept.
    pub conflicts: Vec<String>,
    /// `(profile name, error)` of the steps that failed.
    pub failures: Vec<(String, Fault)>,
    /// The version the file has now.
    pub version: u32,
    /// Saving the final file failed.
    pub final_save_failed: Option<Fault>,
}

/// Run the migration against the keychain `store` and the secrets `file`. See the module docs.
pub fn run(store: &dyn SecretStore, file: &dyn SecretStore, input: Input) -> Report {
    let Input { mut config, language, icons, theme, default_source, prefs } = input;
    let old_version = config.version;
    let mut report = Report {
        backup: None,
        save_failed: None,
        keychain: Keychain::Done,
        rekeyed: 0,
        moved: Vec::new(),
        conflicts: Vec::new(),
        failures: Vec::new(),
        version: old_version,
        final_save_failed: None,
    };
    let Some(path) = config.path.clone() else { return report };
    let save = |config: &Config, version: u32| {
        config::save(
            &path,
            Settings { version, language: &language, icons, theme: &theme, default_source, prefs },
            Some(Profiles { connections: &config.connections, folders: &config.folders, last_used: config.last_used }),
        )
    };
    // 1. Backup of a version 1 file (never overwritten).
    if old_version < CONFIG_VERSION {
        let bak = backup_path(&path);
        // Only a backup known not to be there is written (`exists()` would call one that
        // cannot be looked at "not there", and `copy` would replace it).
        match std::fs::symlink_metadata(&bak) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Err(e) = std::fs::copy(&path, &bak) {
                    report.save_failed = Some(Fault::io_at(&e, &bak));
                    return report;
                }
                report.backup = Some(bak);
            }
            Err(e) => {
                report.save_failed = Some(Fault::io_at(&e, &bak));
                return report;
            }
        }
    }
    // 3. The ids first.
    if let Err(e) = save(&config, old_version) {
        report.save_failed = Some(e);
        return report;
    }
    // 4. Probe the store once.
    let probe = config.connections.first().map(|c| c.id.account()).unwrap_or_else(|| "profile:probe".into());
    let keychain_ok = match store.get(&probe) {
        Err(Unavailable(e)) => {
            report.keychain = Keychain::Unavailable(e);
            false
        }
        Ok(_) => true,
    };
    for c in &mut config.connections {
        let account = c.id.account();
        // 4. The entry of a version 1 file, keyed by the profile name.
        if keychain_ok && old_version < CONFIG_VERSION {
            match rekey(store, &c.name, &account) {
                Ok(Rekey::Moved) => report.rekeyed += 1,
                Ok(Rekey::Conflict) => report.conflicts.push(c.name.clone()),
                Ok(Rekey::Nothing) => {}
                Err(e) => report.failures.push((c.name.clone(), e)),
            }
        }
        // 5. The plaintext password, into the profile's store.
        if !config::has_movable_plaintext(c) {
            continue;
        }
        let target = match c.password_source.unwrap_or(SourceKind::Keychain) {
            SourceKind::File => file,
            _ if keychain_ok => store,
            _ => continue,
        };
        match target.get(&account) {
            Ok(Some(existing)) if existing == c.password => {
                c.password.clear();
                report.moved.push(c.id);
            }
            Ok(Some(_)) => report.conflicts.push(c.name.clone()),
            Ok(None) => match store_verified(target, &account, &c.password) {
                Ok(()) => {
                    c.password.clear();
                    report.moved.push(c.id);
                }
                Err(e) => report.failures.push((c.name.clone(), store_error(e))),
            },
            Err(Unavailable(e)) => report.failures.push((c.name.clone(), e)),
        }
    }
    if !report.failures.is_empty() {
        report.keychain = Keychain::Partial;
    }
    // 7. Done: record the version (a partial keychain move keeps the old one to retry).
    let version = if report.keychain == Keychain::Partial { old_version } else { CONFIG_VERSION };
    let changed = version != old_version || !report.moved.is_empty();
    if changed {
        match save(&config, version) {
            Ok(()) => report.version = version,
            Err(e) => report.final_save_failed = Some(e),
        }
    }
    report
}

enum Rekey {
    Nothing,
    Moved,
    Conflict,
}

/// Move the entry `old` (a profile name) to `new` (`profile:<uuid>`), never overwriting `new`.
fn rekey(store: &dyn SecretStore, old: &str, new: &str) -> Result<Rekey, Fault> {
    let Some(pw) = store.get(old).map_err(|Unavailable(e)| e)? else { return Ok(Rekey::Nothing) };
    match store.get(new).map_err(|Unavailable(e)| e)? {
        Some(existing) if existing == pw => {
            store.delete(old).map_err(|Unavailable(e)| e)?;
            Ok(Rekey::Moved)
        }
        // Maybe a newer password the user saved since: keep both.
        Some(_) => Ok(Rekey::Conflict),
        None => {
            store_verified(store, new, &pw).map_err(store_error)?;
            store.delete(old).map_err(|Unavailable(e)| e)?;
            Ok(Rekey::Moved)
        }
    }
}

fn store_error(e: StoreError) -> Fault {
    match e {
        StoreError::Unavailable(e) => e,
        StoreError::Mismatch => {
            Fault::new(FaultKind::Keychain(KeychainFault::Mismatch), "the keychain gave back a different password")
        }
    }
}

/// `config.toml` -> `config.toml.v1.bak`.
pub fn backup_path(path: &std::path::Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".v1.bak");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests;
