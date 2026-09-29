//! File helpers shared by everything datarig writes.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replace the file at `path` with `bytes` so that a crash or a full disk never leaves a
/// half-written file:
///
/// 1. a symbolic link (dotfiles) is written through to its target;
/// 2. the bytes go to `.<name>.tmp-<pid>` in the same directory, which gets the original
///    file's permissions (Unix mode), and are flushed to disk;
/// 3. the temporary file is renamed over the target (`rename` replaces it on Windows too),
///    and on Unix the directory is synced so the rename itself survives a crash;
/// 4. on failure the temporary file is removed and the error returned; the target is untouched.
///
/// Missing parent directories are created.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomically(path, bytes, false)
}

/// [`atomic_write`] for files that hold secrets: the file is always written with mode 0600
/// (Unix), whatever the permissions of the file it replaces. On Windows it is the same as
/// [`atomic_write`] (the file inherits the directory's ACL).
pub fn atomic_write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomically(path, bytes, true)
}

fn write_atomically(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let target = write_target(path)?;
    let dir = match target.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.to_path_buf(),
        None => PathBuf::from("."),
    };
    fs::create_dir_all(&dir)?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = write_then_rename(&tmp, &target, bytes, private);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    sync_dir(&dir);
    Ok(())
}

fn write_then_rename(tmp: &Path, target: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    // A private file is never readable by others, not even for a moment.
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    }
    let mut f = opts.open(tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if private {
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        } else if let Ok(meta) = fs::metadata(target) {
            f.set_permissions(meta.permissions())?;
        }
    }
    #[cfg(not(unix))]
    let _ = (target, private);
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(tmp, target)
}

/// Write a new file at `path`, never replacing anything already there (an `AlreadyExists`
/// error then). The bytes go to a temporary file first, which is flushed and then hard-linked
/// into place, so the file appears complete or not at all and an existing one is never
/// touched. On a file system without hard links the file is created with `create_new`
/// (`O_EXCL`) and written in place (removed again when that fails). Missing parent
/// directories are created.
pub fn create_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.to_path_buf(),
        None => PathBuf::from("."),
    };
    fs::create_dir_all(&dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.new-{}", std::process::id()));
    let result = (|| {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        match fs::hard_link(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
            // No hard links here: create it exclusively and write it in place.
            Err(_) => {
                let mut f = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
                let written = f.write_all(bytes).and_then(|()| f.sync_all());
                if written.is_err() {
                    let _ = fs::remove_file(path);
                }
                written
            }
        }
    })();
    let _ = fs::remove_file(&tmp);
    if result.is_ok() {
        sync_dir(&dir);
    }
    result
}

/// Rename `from` to `to`, never replacing anything at `to` (an `AlreadyExists` error then;
/// `fs::rename` alone would silently replace a file). A file is hard-linked to its new name
/// and then unlinked from the old one; a folder, a symbolic link, or a file on a file system
/// without hard links is renamed after checking that `to` is free (an error while checking is
/// an error, never "free").
pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_file() {
        match fs::hard_link(from, to) {
            Ok(()) => {
                if let Err(e) = fs::remove_file(from) {
                    let _ = fs::remove_file(to);
                    return Err(e);
                }
                if let Some(d) = to.parent() {
                    sync_dir(d);
                }
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {}
        }
    }
    match fs::symlink_metadata(to) {
        Ok(_) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::rename(from, to)
}

/// The file a write of `path` goes to: `path` itself, or the file a symbolic link at `path`
/// points to (a dotfiles link is written through, never replaced by a plain file). A link that
/// cannot be resolved for another reason than a missing target is an error.
fn write_target(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(t) => Ok(t),
        Err(e) => match fs::symlink_metadata(path) {
            // A link to a file that does not exist yet: create that file.
            Ok(m) if m.file_type().is_symlink() && e.kind() == io::ErrorKind::NotFound => {
                let to = fs::read_link(path)?;
                Ok(match path.parent() {
                    Some(dir) if to.is_relative() => dir.join(to),
                    _ => to,
                })
            }
            Ok(m) if m.file_type().is_symlink() => Err(e),
            // Not there yet (or not a link): write the path itself.
            _ => Ok(path.to_path_buf()),
        },
    }
}

/// Where to move a file aside as a backup: `path` when nothing is there, else the first free
/// `<path>.<n>` (n = 2, 3, …), so an older backup is never replaced. A name that cannot be
/// looked at counts as taken. Move with [`rename_new`], which refuses a taken name anyway.
pub fn free_backup(path: &Path) -> PathBuf {
    let taken = |p: &Path| !matches!(fs::symlink_metadata(p), Err(e) if e.kind() == io::ErrorKind::NotFound);
    if !taken(path) {
        return path.to_path_buf();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    (2..).map(|n| path.with_file_name(format!("{name}.{n}"))).find(|p| !taken(p)).unwrap_or_default()
}

/// Make a rename in `dir` durable (Unix). Best effort: some file systems refuse it, and the
/// data itself is already on disk.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests;
