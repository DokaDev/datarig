//! Result rows on disk ("spill"): one file per result in `<state>/spill/`,
//! created `0600` (the rows are query results), deleted when the result goes away.
//!
//! * `datarig-spill-<pid>.lock`: held with an OS file lock by process `<pid>` while it has
//!   spill files; `datarig-spill-<pid>-<n>.rows`: the rows of one result.
//! * A row is a varint cell count, then per cell a varint (0: NULL, n + 1: n bytes of UTF-8
//!   follow) and the bytes. The offset of every [`BLOCK`]th row is kept in memory, so row `i`
//!   is found by reading from the start of its block (8 bytes per `BLOCK` rows).
//! * [`sweep`] deletes the files a crashed run left behind, and only those: names of this
//!   format, regular files, of a process id that is not running and whose lock nobody holds.
//! * The directory is created `0700`. One that is there already with broader permissions is
//!   restricted to `0700` when this user owns it; one another user owns, or one that cannot be
//!   restricted, is refused (nothing spills: [`FaultKind::NotPrivate`]).

use crate::driver::Cell;
use crate::fault::{Fault, FaultKind};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Rows per entry of a file's in-memory index.
pub const BLOCK: usize = 256;

const PREFIX: &str = "datarig-spill-";

/// The spill directory of one process: `<state>/spill`. Nothing is created until the first
/// result spills.
pub struct SpillDir {
    dir: PathBuf,
    /// The lock of this process, once it spilled, and whether this handle took it (another
    /// `SpillDir` of the same process on the same directory may hold it).
    lock: Mutex<Option<(File, PathBuf, bool)>>,
}

/// The number of the next spill file of this process (unique across its `SpillDir`s).
static NEXT: AtomicU64 = AtomicU64::new(1);

impl SpillDir {
    pub fn new(state: &Path) -> Self {
        Self { dir: dir_of(state), lock: Mutex::new(None) }
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// A new, empty spill file.
    pub fn create(&self) -> io::Result<SpillFile> {
        {
            let mut lock = self.lock.lock().map_err(|_| io::Error::other("spill lock poisoned"))?;
            if lock.is_none() {
                create_private_dir(&self.dir)?;
                let path = self.dir.join(format!("{PREFIX}{}.lock", std::process::id()));
                let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
                let owned = match file.try_lock() {
                    Ok(()) => true,
                    // A file system without locks: the process id in the name protects it.
                    Err(fs::TryLockError::Error(e)) if e.kind() == io::ErrorKind::Unsupported => true,
                    // The name has this process's id: another handle of this process holds it.
                    Err(fs::TryLockError::WouldBlock) => false,
                    Err(fs::TryLockError::Error(e)) => return Err(e),
                };
                *lock = Some((file, path, owned));
            }
        }
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = self.dir.join(format!("{PREFIX}{}-{n}.rows", std::process::id()));
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = opts.open(&path)?;
        Ok(SpillFile { path, file: Some(file), len: 0, rows: 0, blocks: Vec::new() })
    }
}

impl Drop for SpillDir {
    fn drop(&mut self) {
        if let Ok(mut lock) = self.lock.lock()
            && let Some((file, path, owned)) = lock.take()
        {
            // Unlocked first (Windows cannot delete an open file); a sweep of another process
            // leaves it alone meanwhile, as this process is still running.
            drop(file);
            if owned {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

fn dir_of(state: &Path) -> PathBuf {
    state.join("spill")
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)?;
    #[cfg(unix)]
    make_private(dir, rustix::process::geteuid().as_raw())?;
    Ok(())
}

/// Make `dir`, which may have been there already (an older version, another tool, a lax
/// umask), private to user `uid`: restricted to `0700` when `uid` owns it, refused
/// ([`NotPrivate`]) when another user does or it cannot be restricted.
#[cfg(unix)]
fn make_private(dir: &Path, uid: u32) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = fs::metadata(dir)?;
    if meta.uid() != uid {
        return Err(NotPrivate::error(format!("owned by user {}, not {uid}", meta.uid())));
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| NotPrivate::error(format!("mode {:o} could not be restricted: {e}", meta.mode() & 0o777)))?;
    }
    Ok(())
}

/// The spill directory is not private to this user (see [`make_private`]).
#[derive(Debug)]
struct NotPrivate(String);

impl NotPrivate {
    #[cfg(unix)]
    fn error(why: String) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, NotPrivate(why))
    }
}

impl std::fmt::Display for NotPrivate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the spill directory is not private: {}", self.0)
    }
}

impl std::error::Error for NotPrivate {}

/// A failure of [`SpillDir::create`] on `path` as a fault: [`FaultKind::NotPrivate`] for a
/// directory that is not private, else the io kind.
pub fn create_fault(e: &io::Error, path: &Path) -> Fault {
    if e.get_ref().is_some_and(|inner| inner.is::<NotPrivate>()) {
        return Fault::new(FaultKind::NotPrivate, format!("{}: {e}", path.display()));
    }
    Fault::io_at(e, path)
}

/// The rows of one result on disk. The file is deleted when this is dropped.
pub struct SpillFile {
    path: PathBuf,
    /// Closed before the file is deleted (Windows cannot delete an open file).
    file: Option<File>,
    /// Bytes written.
    len: u64,
    rows: usize,
    /// The offset of row `i * BLOCK`.
    blocks: Vec<u64>,
}

impl Drop for SpillFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn get_varint(buf: &[u8], at: &mut usize) -> io::Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*at).ok_or_else(|| corrupt("truncated varint"))?;
        *at += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(corrupt("varint too long"))
}

fn corrupt(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("spill file: {what}"))
}

/// Append the encoding of `row` to `out`.
pub fn encode_row(out: &mut Vec<u8>, row: &[Cell]) {
    put_varint(out, row.len() as u64);
    for cell in row {
        match cell {
            None => put_varint(out, 0),
            Some(s) => {
                put_varint(out, s.len() as u64 + 1);
                out.extend_from_slice(s.as_bytes());
            }
        }
    }
}

fn decode_row(buf: &[u8], at: &mut usize) -> io::Result<Vec<Cell>> {
    let n = get_varint(buf, at)?;
    let n = usize::try_from(n).map_err(|_| corrupt("cell count"))?;
    let mut row = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let tag = get_varint(buf, at)?;
        if tag == 0 {
            row.push(None);
            continue;
        }
        let len = usize::try_from(tag - 1).map_err(|_| corrupt("cell length"))?;
        let end = at.checked_add(len).filter(|e| *e <= buf.len()).ok_or_else(|| corrupt("truncated cell"))?;
        let s = std::str::from_utf8(&buf[*at..end]).map_err(|_| corrupt("cell is not UTF-8"))?;
        row.push(Some(s.to_string()));
        *at = end;
    }
    Ok(row)
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, off)
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, off)
}

#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let n = f.seek_write(&buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        done += n;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let n = f.seek_read(&mut buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
    }
    Ok(())
}

impl SpillFile {
    fn handle(&self) -> io::Result<&File> {
        self.file.as_ref().ok_or_else(|| io::Error::other("spill file closed"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes on disk.
    pub fn bytes(&self) -> u64 {
        self.len
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Append rows in order, as many as keep the file within `cap` bytes; returns how many.
    pub fn append(&mut self, rows: &[Vec<Cell>], cap: Option<u64>) -> io::Result<usize> {
        let mut buf = Vec::new();
        let mut blocks = Vec::new();
        let mut kept = 0;
        for row in rows {
            let before = buf.len();
            encode_row(&mut buf, row);
            if cap.is_some_and(|c| self.len + buf.len() as u64 > c) {
                buf.truncate(before);
                break;
            }
            if (self.rows + kept).is_multiple_of(BLOCK) {
                blocks.push(self.len + before as u64);
            }
            kept += 1;
        }
        write_at(self.handle()?, &buf, self.len)?;
        self.len += buf.len() as u64;
        self.rows += kept;
        self.blocks.extend(blocks);
        Ok(kept)
    }

    /// Rows `range` (within the rows written).
    pub fn read(&self, range: Range<usize>) -> io::Result<Vec<Vec<Cell>>> {
        let (a, b) = (range.start, range.end.min(self.rows));
        if a >= b {
            return Ok(Vec::new());
        }
        let first = a / BLOCK;
        let from = self.blocks[first];
        let to = self.blocks.get(b.div_ceil(BLOCK)).copied().unwrap_or(self.len);
        let mut buf = vec![0u8; usize::try_from(to - from).map_err(|_| corrupt("range too large"))?];
        read_at(self.handle()?, &mut buf, from)?;
        let mut at = 0;
        let mut out = Vec::with_capacity(b - a);
        for i in first * BLOCK..b {
            let row = decode_row(&buf, &mut at)?;
            if i >= a {
                out.push(row);
            }
        }
        Ok(out)
    }
}

/// What [`sweep`] did.
#[derive(Debug, Default)]
pub struct Sweep {
    /// Files deleted (left behind by runs that are gone).
    pub removed: Vec<PathBuf>,
    /// Files it could not look at or delete; nothing is assumed about them.
    pub failed: Vec<(PathBuf, io::Error)>,
}

/// A spill file name: the owner's process id, and whether it is the owner's lock.
fn parse_name(name: &str) -> Option<(u32, bool)> {
    let rest = name.strip_prefix(PREFIX)?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(pid) = rest.strip_suffix(".lock") {
        return digits(pid).then(|| pid.parse().ok()).flatten().map(|p| (p, true));
    }
    let (pid, n) = rest.strip_suffix(".rows")?.split_once('-')?;
    (digits(pid) && digits(n)).then(|| pid.parse().ok()).flatten().map(|p| (p, false))
}

/// Delete the spill files of runs that are gone: in `<state>/spill`, only regular files named
/// as spill files, of a process id other than this one that is not running, and whose lock
/// file nobody holds (a held lock means the owner lives, whatever its id says). Anything it
/// cannot tell is left alone and reported.
pub fn sweep(state: &Path) -> Sweep {
    let dir = dir_of(state);
    let mut out = Sweep::default();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.failed.push((dir, e));
            return out;
        }
    };
    let mut owners: std::collections::BTreeMap<u32, Vec<(PathBuf, bool)>> = Default::default();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.failed.push((dir.clone(), e));
                continue;
            }
        };
        let Some((pid, lock)) = entry.file_name().to_str().and_then(parse_name) else { continue };
        if pid == std::process::id() {
            continue;
        }
        owners.entry(pid).or_default().push((entry.path(), lock));
    }
    for (pid, files) in owners {
        if crate::workspace::pid_alive(pid) {
            continue;
        }
        let held = match files.iter().find(|(_, lock)| *lock) {
            None => false,
            Some((path, _)) => match File::open(path).map(|f| f.try_lock().map(|()| f)) {
                Ok(Ok(_free)) => false,
                Ok(Err(fs::TryLockError::WouldBlock)) => true,
                Ok(Err(fs::TryLockError::Error(e))) if e.kind() == io::ErrorKind::Unsupported => false,
                Ok(Err(fs::TryLockError::Error(e))) | Err(e) => {
                    out.failed.push((path.clone(), e));
                    true
                }
            },
        };
        if held {
            continue;
        }
        // The lock last: while it exists, a data file of this owner is known to be ours to
        // sweep on the next run too.
        let mut files = files;
        files.sort_by_key(|(_, lock)| *lock);
        for (path, _) in files {
            match fs::symlink_metadata(&path) {
                Ok(m) if m.file_type().is_file() => match fs::remove_file(&path) {
                    Ok(()) => out.removed.push(path),
                    Err(e) => out.failed.push((path, e)),
                },
                Ok(_) => {}
                Err(e) => out.failed.push((path, e)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
