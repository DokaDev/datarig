//! Saved queries: plain files under `<data>/scripts/`,
//! and the app's index `<data>/scripts.toml`, which binds each one to the profile (by id) it
//! last ran on. The files stay pure SQL: nothing about connections goes into them.
//!
//! The list is a scan of the directory, so files made or removed outside the app show up.
//! Every write is atomic ([`crate::fsutil::atomic_write`]). A save names the [`Stamp`] of the
//! file as last read or written; when the file changed since (another datarig, an external
//! editor), the save is refused with [`SaveError::Conflict`] and the caller asks the user.
//!
//! Paths given to and returned by the store are relative to the scripts directory and use `/`
//! on every OS.
//!
//! ```toml
//! # scripts.toml
//! version = 1
//! [bindings."reports/daily.sql"]
//! profile = "3f0b8f5e-…"
//! ```

pub mod name;

use crate::fault::Fault;
use crate::fsutil::{atomic_write, create_new, free_backup, rename_new};
use crate::profile::ProfileId;
use name::{NameError, fold};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The extension of SQL scripts (the only language so far).
pub const SQL: &str = "sql";
/// Extensions the list shows.
pub const EXTENSIONS: &[&str] = &[SQL];

/// What identifies the version of a file on disk: its modification time and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub mtime: Option<SystemTime>,
    pub len: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Stamp> {
        let m = fs::metadata(path).ok()?;
        m.is_file().then(|| Stamp { mtime: m.modified().ok(), len: m.len() })
    }
}

/// Why a save did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file changed on disk since it was last read or written (its stamp now).
    Conflict,
    /// The file was removed on disk since it was last read or written.
    Missing,
    /// The name is taken (create).
    Exists(String),
    /// Writing failed: the kind of error (see [`crate::i18n::io_reason`]) and the OS's text.
    Io(io::ErrorKind, String),
}

/// One row of the list: a folder or a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative path (`reports/daily.sql`, `reports`).
    pub path: String,
    pub folder: bool,
    /// A folder whose contents could not be listed: unknown, never taken for an
    /// empty one.
    pub unreadable: bool,
}

impl Entry {
    /// Nesting depth (0 at the top).
    pub fn depth(&self) -> usize {
        self.path.matches('/').count()
    }

    /// The last part of the path, without the extension for a script.
    pub fn name(&self) -> &str {
        display_name(&self.path, self.folder)
    }
}

/// The last part of `path`, without the extension for a script (`reports/daily.sql` →
/// `daily`).
pub fn display_name(path: &str, folder: bool) -> &str {
    let last = path.rsplit('/').next().unwrap_or(path);
    if folder {
        return last;
    }
    match last.rfind('.') {
        Some(i) if i > 0 => &last[..i],
        _ => last,
    }
}

/// A script's path without its extension (`reports/daily`): what the user types for it.
pub fn stem_path(path: &str) -> &str {
    let last = path.rsplit('/').next().unwrap_or(path);
    match last.rfind('.') {
        Some(i) if i > 0 => &path[..path.len() - (last.len() - i)],
        _ => path,
    }
}

/// The folder of `path` (`None` at the top).
pub fn parent(path: &str) -> Option<&str> {
    path.rfind('/').map(|i| &path[..i])
}

/// What happened to an index that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexProblem {
    /// Why it could not be read.
    pub error: Fault,
    /// Where it was moved before a new one was written (`scripts.toml.bak`, or the next free
    /// `scripts.toml.bak.<n>`). `None`: it could not be moved, so it is left as it is and not
    /// written this run (bindings are kept in memory only).
    pub backup: Option<PathBuf>,
    /// Bindings recovered from it (of scripts that exist).
    pub kept: usize,
}

pub struct ScriptStore {
    root: PathBuf,
    index: PathBuf,
    bindings: BTreeMap<String, ProfileId>,
    /// The index could not be read at open (see [`IndexProblem`]).
    pub index_problem: Option<IndexProblem>,
    /// The broken index could not be moved aside: never write over it.
    index_frozen: bool,
}

impl ScriptStore {
    /// The store in data directory `data` (`<data>/scripts`, `<data>/scripts.toml`). Nothing is
    /// created until something is saved, except that an index that cannot be read is moved
    /// aside and rebuilt at once (see [`ScriptStore::repair_index`]).
    pub fn open(data: &Path) -> Self {
        let mut s = Self {
            root: data.join("scripts"),
            index: data.join("scripts.toml"),
            bindings: BTreeMap::new(),
            index_problem: None,
            index_frozen: false,
        };
        match fs::read_to_string(&s.index) {
            Ok(text) => match parse_index(&text) {
                Ok(b) => s.bindings = b,
                Err(e) => s.repair_index(e, &text),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                let fault = Fault::io_at(&e, &s.index);
                s.repair_index(fault, "");
            }
        }
        s
    }

    /// The index could not be read (`error`; `text` is what could be read of it): move it to a
    /// free `scripts.toml.bak` name, then write a new one from the scripts on disk with every
    /// binding that can still be read line by line from `text`. When it cannot be moved it is
    /// left alone and never written over this run.
    fn repair_index(&mut self, error: Fault, text: &str) {
        let bak = free_backup(&self.index.with_file_name("scripts.toml.bak"));
        if rename_new(&self.index, &bak).is_err() {
            self.index_frozen = true;
            self.index_problem = Some(IndexProblem { error, backup: None, kept: 0 });
            return;
        }
        self.bindings = salvage_index(text);
        // Only bindings of scripts that exist are kept (save_index drops the others).
        let _ = self.save_index();
        let kept = self.bindings.len();
        self.index_problem = Some(IndexProblem { error, backup: Some(bak), kept });
    }

    /// The scripts directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file of `rel`.
    pub fn file(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.root.clone(), |p, part| p.join(part))
    }

    /// Every folder and script under the scripts directory: at each level folders first, then
    /// scripts, each by name ignoring case. Hidden files and other extensions are left out.
    pub fn list(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let _ = self.scan(&self.root, "", &mut out);
        out
    }

    /// List `dir` into `out`; `false` when it cannot be read (its entry says so).
    fn scan(&self, dir: &Path, prefix: &str, out: &mut Vec<Entry>) -> bool {
        let Ok(rd) = fs::read_dir(dir) else { return false };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            // A symbolic link counts as what it points to.
            let is_dir = ft.is_dir() || (ft.is_symlink() && e.path().is_dir());
            if is_dir {
                dirs.push(name);
            } else if has_extension(&name) {
                files.push(name);
            }
        }
        dirs.sort_by_key(|n| fold(n));
        files.sort_by_key(|n| fold(n));
        for d in dirs {
            let path = join(prefix, &d);
            let at = out.len();
            out.push(Entry { path: path.clone(), folder: true, unreadable: false });
            if !self.scan(&dir.join(&d), &path, out) {
                out[at].unreadable = true;
            }
        }
        for f in files {
            out.push(Entry { path: join(prefix, &f), folder: false, unreadable: false });
        }
        true
    }

    /// Make the folder `rel` (its name checked by [`ScriptStore::resolve_folder`]), its parents
    /// as needed. It never takes over what is there: a folder or file of that name ignoring case
    /// is `AlreadyExists`, and the folder itself is made exclusively (`create_dir`).
    pub fn create_folder(&self, rel: &str) -> io::Result<()> {
        match self.find(rel) {
            Ok(Some(_)) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            Ok(None) => {}
            Err(u) => return Err(io::Error::new(u.kind, u.detail)),
        }
        let path = self.file(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir(path)
    }

    /// The existing script or folder whose path equals `rel` ignoring case (and Hangul
    /// normalization), as it is spelled on disk. `Ok(None)` only when it is known not to be
    /// there (a folder on the way is missing or is a file); a folder that cannot be read is an
    /// error, never "not there".
    pub fn find(&self, rel: &str) -> Result<Option<String>, Unreadable> {
        let mut dir = self.root.clone();
        let mut spelled = String::new();
        for part in rel.split('/') {
            let want = fold(part);
            let unreadable =
                |e: io::Error| Unreadable { folder: spelled.clone(), kind: e.kind(), detail: e.to_string() };
            let rd = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {
                    return Ok(None);
                }
                Err(e) => return Err(unreadable(e)),
            };
            let mut hit = None;
            for e in rd {
                let name = e.map_err(unreadable)?.file_name().to_string_lossy().into_owned();
                if fold(&name) == want {
                    hit = Some(name);
                    break;
                }
            }
            let Some(hit) = hit else { return Ok(None) };
            dir = dir.join(&hit);
            spelled = join(&spelled, &hit);
        }
        Ok(Some(spelled))
    }

    /// The path a script named `input` gets (see [`name::parse_name`]): folders that exist
    /// (ignoring case) keep their spelling. A name already taken is [`NameError::Exists`],
    /// unless it is `except` (renaming a script to itself in another case).
    pub fn resolve_new(&self, input: &str, except: Option<&str>) -> Result<String, NameError> {
        let rel = name::parse_name(input, SQL)?;
        self.place(&rel, except)
    }

    /// Like [`ScriptStore::resolve_new`] for a folder path.
    pub fn resolve_folder(&self, input: &str, except: Option<&str>) -> Result<String, NameError> {
        let rel = name::parse_folder(input)?;
        self.place(&rel, except)
    }

    fn place(&self, rel: &str, except: Option<&str>) -> Result<String, NameError> {
        let parts: Vec<&str> = rel.split('/').collect();
        let mut spelled = String::new();
        for (i, part) in parts.iter().enumerate() {
            let here = join(&spelled, part);
            let existing = self.find(&here).map_err(|u| NameError::Unreadable(u.folder))?;
            if i + 1 < parts.len() {
                match existing {
                    Some(e) if self.file(&e).is_dir() => spelled = e,
                    Some(e) => return Err(NameError::Exists(e)),
                    None => spelled = here,
                }
            } else {
                match existing {
                    Some(e) if except.is_none_or(|x| fold(x) != fold(&e)) => return Err(NameError::Exists(e)),
                    _ => spelled = here,
                }
            }
        }
        Ok(spelled)
    }

    /// Read script `rel` and its stamp.
    pub fn read(&self, rel: &str) -> io::Result<(String, Stamp)> {
        let path = self.file(rel);
        let text = fs::read_to_string(&path)?;
        let stamp = Stamp::of(&path).unwrap_or(Stamp { mtime: None, len: text.len() as u64 });
        Ok((text, stamp))
    }

    /// The stamp of script `rel` now (`None`: no such file).
    pub fn stamp(&self, rel: &str) -> Option<Stamp> {
        Stamp::of(&self.file(rel))
    }

    /// Write a new script `rel` (its name checked by [`ScriptStore::resolve_new`]). Nothing
    /// that is there is ever replaced: a name taken ignoring case is [`SaveError::Exists`], a
    /// folder on the way that cannot be read is an error, and the file itself is created
    /// exclusively ([`create_new`]).
    pub fn create(&self, rel: &str, text: &str) -> Result<Stamp, SaveError> {
        match self.find(rel) {
            Ok(Some(e)) => return Err(SaveError::Exists(e)),
            Ok(None) => {}
            Err(u) => return Err(SaveError::Io(u.kind, u.detail)),
        }
        let path = self.file(rel);
        match create_new(&path, text.as_bytes()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(SaveError::Exists(rel.to_string())),
            Err(e) => return Err(SaveError::Io(e.kind(), e.to_string())),
        }
        Stamp::of(&path).ok_or_else(|| SaveError::Io(io::ErrorKind::NotFound, path.display().to_string()))
    }

    /// Write script `rel`, last seen as `expected`. A file that changed or went away since is
    /// left alone ([`SaveError::Conflict`] / [`SaveError::Missing`]); `None` writes regardless
    /// (the user chose to overwrite).
    pub fn save(&self, rel: &str, text: &str, expected: Option<Stamp>) -> Result<Stamp, SaveError> {
        if let Some(want) = expected {
            match self.stamp(rel) {
                None => return Err(SaveError::Missing),
                Some(now) if now != want => return Err(SaveError::Conflict),
                Some(_) => {}
            }
        }
        self.write(rel, text)
    }

    fn write(&self, rel: &str, text: &str) -> Result<Stamp, SaveError> {
        let path = self.file(rel);
        atomic_write(&path, text.as_bytes()).map_err(|e| SaveError::Io(e.kind(), e.to_string()))?;
        Stamp::of(&path).ok_or_else(|| SaveError::Io(io::ErrorKind::NotFound, path.display().to_string()))
    }

    /// Rename or move script or folder `from` to `to` (checked already); the bindings follow.
    /// Whatever is at `to` is never replaced (`AlreadyExists`), except by the same file under
    /// another case (renaming `a` to `A` on a file system that ignores case).
    pub fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        let (src, dst) = (self.file(from), self.file(to));
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        if fold(from) == fold(to) && same_entry(&src, &dst)? {
            fs::rename(&src, &dst)?;
        } else {
            rename_new(&src, &dst)?;
        }
        let moved: Vec<(String, ProfileId)> =
            self.bindings.iter().filter(|(k, _)| is_within(k, from)).map(|(k, v)| (k.clone(), *v)).collect();
        for (k, v) in moved {
            self.bindings.remove(&k);
            self.bindings.insert(format!("{to}{}", &k[from.len()..]), v);
        }
        self.save_index()
    }

    /// Delete script `rel` and its binding.
    pub fn delete(&mut self, rel: &str) -> io::Result<()> {
        fs::remove_file(self.file(rel))?;
        self.bindings.remove(rel);
        self.save_index()
    }

    /// Delete the empty folder `rel`.
    pub fn delete_folder(&mut self, rel: &str) -> io::Result<()> {
        fs::remove_dir(self.file(rel))
    }

    /// Whether folder `rel` holds nothing the list shows. A folder that cannot be read is an
    /// error, never "empty".
    pub fn folder_empty(&self, rel: &str) -> io::Result<bool> {
        for e in fs::read_dir(self.file(rel))? {
            if !e?.file_name().to_string_lossy().starts_with('.') {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The profile script `rel` last ran on.
    pub fn binding(&self, rel: &str) -> Option<ProfileId> {
        self.bindings.get(rel).copied()
    }

    /// Bind script `rel` to `profile` (`None`: unbind) and save the index.
    pub fn bind(&mut self, rel: &str, profile: Option<ProfileId>) -> io::Result<()> {
        let old = self.bindings.get(rel).copied();
        if old == profile {
            return Ok(());
        }
        match profile {
            Some(p) => self.bindings.insert(rel.to_string(), p),
            None => self.bindings.remove(rel),
        };
        self.save_index()
    }

    /// Profile `profile` was deleted: unbind every script bound to it (and save the index when
    /// one was).
    pub fn unbind_profile(&mut self, profile: ProfileId) -> io::Result<()> {
        let before = self.bindings.len();
        self.bindings.retain(|_, p| *p != profile);
        if self.bindings.len() == before { Ok(()) } else { self.save_index() }
    }

    /// Write the index; bindings of files that no longer exist are dropped. A file that cannot
    /// be looked at (a directory without permission) keeps its binding: only a file known to
    /// be gone loses it.
    fn save_index(&mut self) -> io::Result<()> {
        let root = self.root.clone();
        self.bindings.retain(|k, _| match fs::metadata(k.split('/').fold(root.clone(), |p, part| p.join(part))) {
            Ok(m) => m.is_file(),
            Err(e) => !matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory),
        });
        let mut doc = toml_edit::DocumentMut::new();
        doc["version"] = toml_edit::value(1);
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        for (k, v) in &self.bindings {
            let mut t = toml_edit::Table::new();
            t["profile"] = toml_edit::value(v.to_string());
            table.insert(k, toml_edit::Item::Table(t));
        }
        doc["bindings"] = toml_edit::Item::Table(table);
        if self.index_frozen {
            return Ok(());
        }
        atomic_write(&self.index, doc.to_string().as_bytes())
    }
}

fn parse_index(text: &str) -> Result<BTreeMap<String, ProfileId>, Fault> {
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| Fault::toml_de(text, &e))?;
    let mut out = BTreeMap::new();
    if let Some(b) = doc.get("bindings").and_then(toml::Value::as_table) {
        for (k, v) in b {
            if let Some(id) = v.get("profile").and_then(toml::Value::as_str).and_then(ProfileId::parse) {
                out.insert(k.clone(), id);
            }
        }
    }
    Ok(out)
}

/// The bindings that can still be read from a broken index, line by line: a `profile = "…"`
/// line under a `[bindings."<path>"]` header, or a `"<path>" = { profile = "…" }` line under
/// `[bindings]`. Everything else is skipped.
fn salvage_index(text: &str) -> BTreeMap<String, ProfileId> {
    // One key as TOML reads it (quoted or bare).
    let key = |k: &str| -> Option<String> {
        let t: toml::Table = format!("{k} = 0").parse().ok()?;
        (t.len() == 1).then(|| t.keys().next().cloned())?
    };
    let profile = |v: &toml::Value| v.get("profile").and_then(toml::Value::as_str).and_then(ProfileId::parse);
    let mut out = BTreeMap::new();
    let mut section: Option<Option<String>> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = match line.strip_prefix("[bindings").and_then(|r| r.strip_suffix(']')) {
                Some("") => Some(None),
                Some(r) => r.strip_prefix('.').and_then(key).map(Some),
                None => None,
            };
            continue;
        }
        let Some(current) = &section else { continue };
        let Ok(t) = line.parse::<toml::Table>() else { continue };
        match current {
            Some(path) => {
                if let Some(id) = profile(&toml::Value::Table(t)) {
                    out.insert(path.clone(), id);
                }
            }
            None => {
                for (path, v) in t {
                    if let Some(id) = profile(&v) {
                        out.insert(path, id);
                    }
                }
            }
        }
    }
    out
}

/// A folder of the scripts directory could not be read, so what is in it is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable {
    /// The folder, relative to the scripts directory (`""`: the scripts directory itself).
    pub folder: String,
    pub kind: io::ErrorKind,
    /// The OS's text (for the log).
    pub detail: String,
}

/// `a` and `b` are the same directory entry (`b` names `a` in another case on a file system
/// that ignores case). `false` when `b` does not exist.
fn same_entry(a: &Path, b: &Path) -> io::Result<bool> {
    let mb = match fs::symlink_metadata(b) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let ma = fs::symlink_metadata(a)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(ma.dev() == mb.dev() && ma.ino() == mb.ino())
    }
    #[cfg(not(unix))]
    {
        // Windows file systems ignore case: the name found ignoring case is this entry.
        let _ = (ma, mb);
        Ok(true)
    }
}

fn has_extension(name: &str) -> bool {
    match name.rfind('.') {
        Some(i) if i > 0 => EXTENSIONS.iter().any(|e| name[i + 1..].eq_ignore_ascii_case(e)),
        _ => false,
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") }
}

/// `path` is `dir` or inside it.
pub fn is_within(path: &str, dir: &str) -> bool {
    path == dir || path.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))
}

#[cfg(test)]
mod tests;
