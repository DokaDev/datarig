//! The deletion log (`<state>/secrets.log`, [`crate::paths::Paths::secrets_log`]): one line
//! for every stored password that datarig removes from the OS keychain or the secrets file,
//! so "why did my keychain item disappear" has an answer. A line names the store and the
//! account (`profile:<uuid>`, or a profile name for a version 1 entry), never the password:
//!
//! ```text
//! 2026-09-25T03:04:05Z deleted source=keychain account=profile:3f0b8f5e-6a57-4f7e-9a53-0c1c2b8f9d11
//! ```
//!
//! Writing is best effort: a log that cannot be written never stops the deletion.

use super::{SecretStore, Unavailable};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Appends deletion lines to a file (or nowhere, without a state directory).
#[derive(Debug, Default)]
pub struct DeletionLog {
    path: Option<PathBuf>,
    /// Serializes appends from the UI thread and background work.
    lock: Mutex<()>,
}

impl DeletionLog {
    /// A log at `path`; `None` writes nothing.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path, lock: Mutex::new(()) }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Append one line for a password removed from `source` under `account`.
    pub fn record(&self, source: &str, account: &str) {
        let Some(path) = &self.path else { return };
        let _guard = self.lock.lock();
        let line = format!("{} deleted source={source} account={account}\n", utc_timestamp(SystemTime::now()));
        let _ = append(path, line.as_bytes());
    }
}

fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(bytes)
}

/// `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub(crate) fn utc_timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Days since 1970-01-01 -> (year, month, day) in the proleptic Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A store whose successful deletions are written to a [`DeletionLog`].
pub struct Logged {
    inner: Arc<dyn SecretStore>,
    source: String,
    log: Arc<DeletionLog>,
}

impl Logged {
    /// `source` names the store in the log (`keychain`, `file`).
    pub fn new(inner: Arc<dyn SecretStore>, source: &str, log: Arc<DeletionLog>) -> Self {
        Self { inner, source: source.to_string(), log }
    }
}

impl SecretStore for Logged {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        self.inner.get(account)
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.inner.set(account, secret)
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        let removed = self.inner.delete(account)?;
        if removed {
            self.log.record(&self.source, account);
        }
        Ok(removed)
    }
}
