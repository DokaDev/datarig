//! Where datarig keeps its files. The config file keeps its own rules
//! ([`crate::config::default_path`]); this module computes the data directory (saved queries)
//! and the state directory (tab restore, console buffers, the instance lock, one-time notices,
//! the log of deleted passwords).
//!
//! Precedence, for each directory: the `DATARIG_DATA_DIR` / `DATARIG_STATE_DIR` override,
//! then `$XDG_DATA_HOME/datarig` / `$XDG_STATE_HOME/datarig` when set (on every OS, so a
//! development run pointed at scratch XDG directories never touches the real ones), then the
//! platform default:
//!
//! | | Linux and other Unix | macOS | Windows |
//! |---|---|---|---|
//! | data | `~/.local/share/datarig` | `~/Library/Application Support/datarig` | `%APPDATA%\datarig` |
//! | state | `~/.local/state/datarig` | `~/Library/Application Support/datarig/state` | `%LOCALAPPDATA%\datarig` |

use std::path::{Path, PathBuf};

/// The platform whose default directories apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Unix,
    MacOs,
    Windows,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Unix
        }
    }
}

/// The app's data and state directories. `None` when neither an override nor a home
/// directory is known (nothing is written there then).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paths {
    pub data: Option<PathBuf>,
    pub state: Option<PathBuf>,
}

impl Paths {
    /// From the process environment.
    pub fn from_env() -> Self {
        Self::resolve(|k| std::env::var(k).ok(), Os::current())
    }

    /// From `env` on `os` (tests pass both).
    pub fn resolve(env: impl Fn(&str) -> Option<String>, os: Os) -> Self {
        let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = || var("HOME").or_else(|| var("USERPROFILE"));
        let xdg = |k: &str| var(k).map(|d| d.join("datarig"));
        let data = var("DATARIG_DATA_DIR").or_else(|| xdg("XDG_DATA_HOME")).or_else(|| match os {
            Os::Unix => home().map(|h| h.join(".local").join("share").join("datarig")),
            Os::MacOs => home().map(|h| mac_support(&h)),
            Os::Windows => var("APPDATA").map(|d| d.join("datarig")),
        });
        let state = var("DATARIG_STATE_DIR").or_else(|| xdg("XDG_STATE_HOME")).or_else(|| match os {
            Os::Unix => home().map(|h| h.join(".local").join("state").join("datarig")),
            Os::MacOs => home().map(|h| mac_support(&h).join("state")),
            Os::Windows => var("LOCALAPPDATA").map(|d| d.join("datarig")),
        });
        Paths { data, state }
    }

    /// `<state>/notices.toml`: notices that are shown once per machine.
    pub fn notices_file(&self) -> Option<PathBuf> {
        self.state.as_ref().map(|s| s.join("notices.toml"))
    }

    /// `<state>/secrets.log`: one line per stored password datarig deleted
    /// ([`crate::secret::DeletionLog`]).
    pub fn secrets_log(&self) -> Option<PathBuf> {
        self.state.as_ref().map(|s| s.join("secrets.log"))
    }

    /// `<state>/errors.log`: the raw detail of failures the UI reported in words
    /// ([`crate::fault::ErrorLog`]).
    pub fn errors_log(&self) -> Option<PathBuf> {
        self.state.as_ref().map(|s| s.join("errors.log"))
    }

    /// Whether the one-time notice `key` was not shown on this machine yet; records that it
    /// is now. Without a state directory every run counts as the first. A file that cannot be
    /// read or parsed is never written over: the notice shows, and is not recorded.
    pub fn first_time(&self, key: &str) -> bool {
        let Some(file) = self.notices_file() else { return true };
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return true,
        };
        let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else { return true };
        if doc.get(key).and_then(toml_edit::Item::as_bool) == Some(true) {
            return false;
        }
        doc[key] = toml_edit::value(true);
        let _ = crate::fsutil::atomic_write(&file, doc.to_string().as_bytes());
        true
    }
}

fn mac_support(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("datarig")
}

#[cfg(test)]
mod tests;
