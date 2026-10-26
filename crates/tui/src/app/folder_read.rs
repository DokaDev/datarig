//! The folder reads of the file-system pickers (the SSH key file, where an export goes): every
//! one runs on a worker, never on the UI thread, so a folder on a mount that does not answer
//! (NFS, SMB, sshfs) cannot freeze the app. Each read has a number of its own; its answer comes
//! back as [`AppEvent::Folder`] (headless: [`App::pump_folder_reads`]), and one the picker no
//! longer waits for (it closed, showed another folder, or stopped waiting) is dropped. A read
//! that does not answer within the limit ([`App::set_folder_timeout`], 10 s, as a keychain
//! call) is answered [`NoAnswer::TooSlow`]: its thread is left behind, and what it did not say
//! is never taken for an empty folder. One path is read by one thread at a time: a read of a
//! path whose earlier read has not returned waits for it (within its own limit) instead of
//! starting another, so a mount that does not answer holds one thread per path, not one per
//! key pressed.
//!
//! The reads go through a [`FileSystem`] ([`App::set_file_system`]; tests put in one that
//! hangs). Only folders are listed and kinds asked for (`stat`); no file is opened.

use super::*;
use datarig_core::fault::Fault;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, mpsc};

/// How long a folder read may take.
pub const FOLDER_TIMEOUT: Duration = Duration::from_secs(10);

/// What an entry of a folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    File,
    /// Its kind could not be asked for: neither taken for a file nor for a folder.
    Unknown,
}

/// The reads the pickers make of the file system.
pub trait FileSystem: Send + Sync {
    /// The entries of folder `dir` (a link as what it leads to), in any order.
    fn list(&self, dir: &Path) -> std::io::Result<Vec<(String, Kind)>>;
    /// Folder `dir` can be listed.
    fn readable(&self, dir: &Path) -> std::io::Result<()>;
    /// `path` is a folder (or a link to one).
    fn is_dir(&self, path: &Path) -> bool;
    /// `path` itself is a link.
    fn is_link(&self, path: &Path) -> bool;
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
    /// Make folder `path` (its parent exists).
    fn create_dir(&self, path: &Path) -> std::io::Result<()>;
}

/// The real file system.
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn list(&self, dir: &Path) -> std::io::Result<Vec<(String, Kind)>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let Ok(e) = e else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            let kind = match e.file_type() {
                Ok(t) if t.is_symlink() => match std::fs::metadata(e.path()) {
                    Ok(m) if m.is_dir() => Kind::Folder,
                    Ok(_) => Kind::File,
                    Err(_) => Kind::Unknown,
                },
                Ok(t) if t.is_dir() => Kind::Folder,
                Ok(_) => Kind::File,
                Err(_) => Kind::Unknown,
            };
            out.push((name, kind));
        }
        Ok(out)
    }

    fn readable(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::read_dir(dir).map(|_| ())
    }

    fn is_dir(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_dir())
    }

    fn is_link(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn create_dir(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }
}

/// The entries of `dir` as the pickers list them: folders first, then the rest, each by name
/// (case folded).
pub(super) fn sorted(fs: &dyn FileSystem, dir: &Path) -> Result<Vec<(String, Kind)>, Fault> {
    let mut out = fs.list(dir).map_err(|e| Fault::io_at(&e, dir))?;
    out.sort_by(|a, b| {
        (b.1 == Kind::Folder).cmp(&(a.1 == Kind::Folder)).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    Ok(out)
}

/// What a read found.
#[derive(Debug)]
pub enum Got {
    /// The folder to show (the first of those asked for that is a folder, else the last), and
    /// what it holds.
    Shown { root: PathBuf, select: Option<String>, items: Result<Vec<(String, Kind)>, Fault> },
    /// What a folder inside holds.
    Listed(Result<Vec<(String, Kind)>, Fault>),
    /// The folder is a link back to a folder on the way to it: not listed.
    Loops,
    /// Whether the folder can be listed, and whether the path the field names is a folder.
    Readable { result: Result<(), Fault>, typed_dir: bool },
    /// Whether a typed path is a folder.
    IsDir(bool),
    /// A new folder was made, or why not.
    Made(Result<(), Fault>),
}

/// A read that gave no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoAnswer {
    /// Not within the limit.
    TooSlow,
    /// It ended without one (it failed inside).
    Broke,
}

/// The answer of read `id`.
#[derive(Debug)]
pub struct FolderAnswer {
    pub id: u64,
    pub got: Result<Got, NoAnswer>,
}

/// Where answers go: the event loop, or (headless) the app's own channel.
#[derive(Clone)]
enum Out {
    App(tokio::sync::mpsc::UnboundedSender<AppEvent>),
    Local(mpsc::Sender<FolderAnswer>),
}

/// Starts reads; a picker keeps one.
#[derive(Clone)]
pub struct FolderReader {
    fs: Arc<dyn FileSystem>,
    timeout: Duration,
    out: Out,
    next: Arc<AtomicU64>,
    /// Headless reads not handed to the app yet.
    pending: Arc<AtomicUsize>,
    /// Paths a thread is reading now (one at a time each).
    busy: Busy,
}

/// The paths being read, and a signal when one is done.
type Busy = Arc<(Mutex<HashSet<PathBuf>>, Condvar)>;

impl FolderReader {
    /// Run `work` (a read of `path`) on a worker; its answer (or [`NoAnswer`]) comes back
    /// under the number given.
    pub fn ask(&self, path: PathBuf, work: impl FnOnce(&dyn FileSystem) -> Got + Send + 'static) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (fs, timeout, out, busy) = (self.fs.clone(), self.timeout, self.out.clone(), self.busy.clone());
        if matches!(out, Out::Local(_)) {
            self.pending.fetch_add(1, Ordering::SeqCst);
        }
        // A waiter that answers within the limit either way, and (once no other thread reads
        // `path`) a reader, which may hang for good.
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + timeout;
            let got = match take(&busy, &path, deadline) {
                false => Err(NoAnswer::TooSlow),
                true => {
                    let (tx, rx) = mpsc::channel();
                    let done = busy.clone();
                    std::thread::spawn(move || {
                        // Given back even if `work` fails inside.
                        let _turn = Turn(done, path);
                        let _ = tx.send(work(&*fs));
                    });
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    rx.recv_timeout(left).map_err(|e| match e {
                        mpsc::RecvTimeoutError::Timeout => NoAnswer::TooSlow,
                        mpsc::RecvTimeoutError::Disconnected => NoAnswer::Broke,
                    })
                }
            };
            let answer = FolderAnswer { id, got };
            match out {
                Out::App(tx) => {
                    let _ = tx.send(AppEvent::Folder(answer));
                }
                Out::Local(tx) => {
                    let _ = tx.send(answer);
                }
            }
        });
        id
    }
}

/// Wait until no thread reads `path` (`false`: not before `deadline`), then mark it read.
fn take(busy: &Busy, path: &Path, deadline: std::time::Instant) -> bool {
    let (set, cv) = &**busy;
    let mut set = set.lock().unwrap_or_else(|e| e.into_inner());
    while set.contains(path) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        set = cv.wait_timeout(set, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    set.insert(path.to_path_buf());
    true
}

/// A reader's hold on its path, given back when it ends.
struct Turn(Busy, PathBuf);

impl Drop for Turn {
    fn drop(&mut self) {
        let (set, cv) = &*self.0;
        set.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.1);
        cv.notify_all();
    }
}

/// The app's side of the reads.
pub struct FolderReads {
    fs: Arc<dyn FileSystem>,
    timeout: Duration,
    next: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    busy: Busy,
    local: Option<(mpsc::Sender<FolderAnswer>, mpsc::Receiver<FolderAnswer>)>,
}

impl Default for FolderReads {
    fn default() -> Self {
        Self {
            fs: Arc::new(RealFileSystem),
            timeout: FOLDER_TIMEOUT,
            next: Arc::default(),
            pending: Arc::default(),
            busy: Arc::default(),
            local: None,
        }
    }
}

/// Folder `rel` (relative to `root`) is a link to a folder on the way to it (`root`, or one
/// between it and the link, as they really are: through any number of links).
pub(super) fn loops(fs: &dyn FileSystem, root: &Path, rel: &str) -> bool {
    let dir = root.join(rel);
    if !fs.is_link(&dir) {
        return false;
    }
    let Ok(target) = fs.canonicalize(&dir) else { return false };
    let mut way = vec![root.to_path_buf()];
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        let next = way[way.len() - 1].join(part);
        way.push(next);
    }
    way.iter().filter_map(|p| fs.canonicalize(p).ok()).any(|p| p.starts_with(&target))
}

impl App {
    /// The file system the pickers read (tests put in one that hangs).
    pub fn set_file_system(&mut self, fs: Arc<dyn FileSystem>) {
        self.folder_reads.fs = fs;
    }

    /// Folder reads end within `timeout` (tests make it short).
    pub fn set_folder_timeout(&mut self, timeout: Duration) {
        self.folder_reads.timeout = timeout;
    }

    /// A reader whose answers come back to this app.
    pub(super) fn folder_reader(&mut self) -> FolderReader {
        let r = &mut self.folder_reads;
        let out = match &self.tx {
            Some(tx) => Out::App(tx.clone()),
            None => Out::Local(r.local.get_or_insert_with(mpsc::channel).0.clone()),
        };
        FolderReader {
            fs: r.fs.clone(),
            timeout: r.timeout,
            out,
            next: r.next.clone(),
            pending: r.pending.clone(),
            busy: r.busy.clone(),
        }
    }

    /// Headless (without the event loop's channel): hand the answers of the folder reads to the
    /// app, waiting up to `wait` while one is still out (an answer may ask for another). How
    /// many came.
    pub fn pump_folder_reads(&mut self, wait: Duration) -> usize {
        let deadline = std::time::Instant::now() + wait;
        let mut n = 0;
        while self.folder_reads.pending.load(Ordering::SeqCst) > 0 {
            let Some((_, rx)) = &self.folder_reads.local else { break };
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(answer) = rx.recv_timeout(left) else { break };
            self.folder_reads.pending.fetch_sub(1, Ordering::SeqCst);
            n += 1;
            self.on_app_event(AppEvent::Folder(answer));
        }
        n
    }

    /// A folder read answered: to the export dialog's `Tab` or the picker that asked, unless
    /// neither waits for it any more.
    pub(super) fn on_folder_answer(&mut self, a: FolderAnswer) {
        if self.overlays.export_form().and_then(|f| f.completion.as_ref()).is_some_and(|c| c.wait == Some(a.id)) {
            return self.export_path_completed(a.got);
        }
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let Some(what) = t.disk.answered(a.id) else { return };
        self.tree_answer(what, a.got);
    }
}
