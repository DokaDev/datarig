//! The secrets file (`password_source = "file"`): `secrets.toml` in the
//! directory of the config file, **separate from `config.toml`** so the config can live in a
//! dotfiles repository. Passwords are keyed by profile id:
//!
//! ```toml
//! # datarig passwords (password_source = "file"), keyed by profile id.
//! [passwords]
//! "3f0b8f5e-6a57-4f7e-9a53-0c1c2b8f9d11" = "…"
//! ```
//!
//! * The file is created with mode 0600 and every write is atomic
//!   ([`crate::fsutil::atomic_write_private`]).
//! * On Unix a file that the group or others may read or write (any of the 0o077 bits) is
//!   **refused**: nothing is read from it or written to it until `chmod 600` fixes it
//!   ([`FileError::Insecure`]).
//! * On Windows there are no mode bits: the file relies on the ACL of the user's profile
//!   directory (it is created in `%USERPROFILE%\.config\datarig` by default), which datarig
//!   does not check.

use super::{SecretStore, Unavailable};
use crate::fault::{Fault, FaultKind};
use std::fmt;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

/// The file name, next to `config.toml`.
pub const FILE_NAME: &str = "secrets.toml";

const HEADER: &str = "# datarig passwords (password_source = \"file\"), keyed by profile id.\n\
                      # Keep this file private: chmod 600. Never commit it.\n";

/// Why the secrets file cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileError {
    /// Unix: the group or others have access (`mode` is the permission bits).
    Insecure { path: PathBuf, mode: u32 },
    /// Reading, parsing or writing failed.
    Io(Fault),
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::Insecure { path, mode } => {
                write!(f, "{} has permissions {mode:03o}; run: chmod 600 {}", path.display(), path.display())
            }
            FileError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// `Err(Insecure)` when the file exists and the group or others have any access (Unix). A
/// missing file is fine (it is created 0600).
pub fn check_permissions(path: &Path) -> Result<(), FileError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(m) if m.permissions().mode() & 0o077 != 0 => {
                return Err(FileError::Insecure { path: path.to_path_buf(), mode: m.permissions().mode() & 0o777 });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(FileError::Io(Fault::io_at(&e, path))),
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The key of `account` in the file: the profile id (`profile:` dropped).
fn key(account: &str) -> &str {
    account.strip_prefix("profile:").unwrap_or(account)
}

/// [`SecretStore`] on the secrets file.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `<dir of config_path>/secrets.toml`.
    pub fn next_to(config_path: &Path) -> PathBuf {
        config_path.parent().map_or_else(|| PathBuf::from(FILE_NAME), |d| d.join(FILE_NAME))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The document (empty when the file does not exist), after the permission check.
    fn read(&self) -> Result<DocumentMut, FileError> {
        check_permissions(&self.path)?;
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!("{HEADER}[passwords]\n"),
            Err(e) => return Err(FileError::Io(Fault::io_at(&e, &self.path))),
        };
        text.parse::<DocumentMut>().map_err(|e| FileError::Io(Fault::toml_edit(&text, &e)))
    }

    fn write(&self, doc: &DocumentMut) -> Result<(), FileError> {
        crate::fsutil::atomic_write_private(&self.path, doc.to_string().as_bytes())
            .map_err(|e| FileError::Io(Fault::io_at(&e, &self.path)))
    }

    fn passwords(doc: &DocumentMut) -> Option<&Table> {
        doc.get("passwords").and_then(Item::as_table)
    }
}

fn unavailable(e: FileError) -> Unavailable {
    match e {
        FileError::Io(f) => Unavailable(f),
        // Checked before every use; here only for completeness.
        FileError::Insecure { .. } => {
            Unavailable(Fault::new(FaultKind::Io(std::io::ErrorKind::PermissionDenied), e.to_string()))
        }
    }
}

impl SecretStore for FileStore {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        let doc = self.read().map_err(unavailable)?;
        Ok(Self::passwords(&doc).and_then(|t| t.get(key(account))).and_then(Item::as_str).map(str::to_string))
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        let mut doc = self.read().map_err(unavailable)?;
        let table = doc.entry("passwords").or_insert_with(|| Item::Table(Table::new()));
        let Some(table) = table.as_table_mut() else {
            let detail = format!("{}: `passwords` must be a table", self.path.display());
            return Err(Unavailable(Fault::new(FaultKind::Shape { key: "passwords".into() }, detail)));
        };
        table.insert(key(account), toml_edit::value(secret));
        self.write(&doc).map_err(unavailable)
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        let mut doc = self.read().map_err(unavailable)?;
        let removed = doc.get_mut("passwords").and_then(Item::as_table_mut).and_then(|t| t.remove(key(account)));
        if removed.is_none() {
            return Ok(false);
        }
        self.write(&doc).map_err(unavailable)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
