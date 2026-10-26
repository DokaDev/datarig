//! The SSH key file picker of the profile form: the tree dialog of the saved
//! queries ([`ScriptTree`] in [`TreeMode::KeyFile`]) over a folder of the file system. It opens
//! at the folder of the field's file, else `~/.ssh` when it exists, else the home folder; `../`
//! (or `←` on the top row) goes up. Hidden files are listed (keys live in `~/.ssh`); likely
//! private keys stand out and public keys, `known_hosts`, `config` and `authorized_keys` are
//! dimmed. Typing filters the listed names; a text with `/` or a leading `~` is a path, which
//! `Enter` takes as it is (`~` is the home folder).
//!
//! Browsing only lists folders and asks for a file's kind (`stat`), on a worker (see
//! `folder_read`); no file is opened, so no key is read. Picking one looks at its mode and name only: a key that others may read (the
//! tunnel refuses it) and a PuTTY key (`.ppk`) are said so below the field, and the text is
//! kept either way.

use super::folder_read::{self, FileSystem, FolderReader, Got, Kind, NoAnswer};
use super::script_tree::{ScriptTree, TreeFocus, TreeMode, TreeRow};
use super::*;
use datarig_core::scripts::Entry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// How the picker shows a file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyLook {
    /// Likely a private key: `*.pem`, `*.key`, `id_*` that is no `.pub`.
    Likely,
    /// Not a private key: `*.pub`, `known_hosts*`, `config`, `authorized_keys*`.
    Dim,
    Other,
}

/// How `name` is shown in the picker.
pub fn key_look(name: &str) -> KeyLook {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".pub")
        || lower.starts_with("known_hosts")
        || lower.starts_with("authorized_keys")
        || lower == "config"
    {
        KeyLook::Dim
    } else if lower.ends_with(".pem") || lower.ends_with(".key") || lower.starts_with("id_") {
        KeyLook::Likely
    } else {
        KeyLook::Other
    }
}

/// The picker's text is a path rather than a filter.
pub fn is_path(text: &str) -> bool {
    text.starts_with('~') || text.contains('/') || text.contains('\\')
}

/// `text` with a leading `~` as `home` (as the tunnel reads the field).
fn expand(text: &str, home: Option<&Path>) -> PathBuf {
    match (text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ if text == "~" => home.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(text)),
        _ => PathBuf::from(text),
    }
}

/// `path` as the field shows it: under `home` as `~/…`.
pub(super) fn shown(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.to_string_lossy().replace('\\', "/")),
        _ => path.to_string_lossy().to_string(),
    }
}

/// Folder `dir` as the picker's top row shows it: `~` for the home folder, `~/…` below it.
pub fn shown_dir(dir: &Path, home: Option<&Path>) -> String {
    if home.is_some_and(|h| h == dir) {
        return "~".to_string();
    }
    shown(dir, home)
}

/// What the picker waits for from a folder read.
#[derive(Clone, Debug)]
pub enum What {
    /// The folder shown: the first of `dirs` that is a folder; `tried` is the one being looked
    /// at (the one that did not answer, if none does in time).
    Show { dirs: Vec<PathBuf>, tried: Arc<AtomicUsize> },
    /// Folder `rel` inside, opened once listed (`select`: then selected).
    Open { rel: String, select: Option<String> },
    /// "Save here" into `dir` (the folder of `rel`).
    SaveHere { rel: String, dir: PathBuf },
    /// A new folder inside `parent` (`dir` on disk).
    NewFolder { parent: Option<String>, dir: PathBuf },
    /// Folder `name` made inside `parent` (`dir`: the new folder on disk).
    Create { parent: Option<String>, name: String, dir: PathBuf },
    /// Whether path `text` typed in the filter (`path` on disk) is a folder.
    Typed { path: PathBuf, text: String },
}

impl What {
    /// The folder whose row says it is being read (`""`: the folder shown; `None`: no row).
    pub(super) fn rel(&self) -> Option<&str> {
        match self {
            What::Show { .. } => Some(""),
            What::Open { rel, .. } | What::SaveHere { rel, .. } => Some(rel),
            What::NewFolder { parent, .. } | What::Create { parent, .. } => Some(parent.as_deref().unwrap_or("")),
            What::Typed { .. } => None,
        }
    }
}

/// The file-system modes' reads: the reader, what is waited for, and what was learned.
#[derive(Clone, Default)]
pub struct Disk {
    reader: Option<FolderReader>,
    /// Reads asked for and not answered yet, by number.
    pub waiting: Vec<(u64, What)>,
    /// Folders whose read took too long (also unreadable until asked again).
    pub slow: BTreeSet<String>,
    /// Entries whose kind could not be told.
    pub unknown: BTreeSet<String>,
}

impl Disk {
    pub fn new(reader: FolderReader) -> Self {
        Self { reader: Some(reader), ..Self::default() }
    }

    /// Read `id` answered: what it was for, unless nobody waits for it any more.
    pub(super) fn answered(&mut self, id: u64) -> Option<What> {
        let i = self.waiting.iter().position(|(n, _)| *n == id)?;
        Some(self.waiting.remove(i).1)
    }

    /// Folder `rel` is being read.
    pub fn reading(&self, rel: &str) -> bool {
        self.waiting.iter().any(|(_, w)| w.rel() == Some(rel))
    }

    /// The folder shown is being read (which one it is, is not known yet).
    pub fn reading_shown(&self) -> bool {
        self.waiting.iter().any(|(_, w)| matches!(w, What::Show { .. }))
    }

    /// A read with no row of its own (a typed path) is out.
    pub fn busy(&self) -> bool {
        self.waiting.iter().any(|(_, w)| w.rel().is_none())
    }
}

impl ScriptTree {
    /// Run `work` (a read of `path`) on a worker for `what`.
    pub(super) fn ask(
        &mut self,
        what: What,
        path: PathBuf,
        work: impl FnOnce(&dyn FileSystem) -> Got + Send + 'static,
    ) {
        let Some(reader) = &self.disk.reader else { return };
        let id = reader.ask(path, work);
        self.disk.waiting.push((id, what));
    }

    /// List folder `rel` (relative to the root) on a worker; it opens (and `select` is
    /// selected) once listed. Export path mode: a link back to a folder on the way is not
    /// listed.
    pub(super) fn load(&mut self, rel: &str, select: Option<String>) {
        if self.disk.waiting.iter().any(|(_, w)| matches!(w, What::Open { rel: r, .. } if r == rel)) {
            return;
        }
        let (root, r) = (self.root.clone(), rel.to_string());
        let guard = matches!(self.mode, TreeMode::ExportPath(_));
        self.ask(What::Open { rel: rel.to_string(), select }, root.join(rel), move |fs| {
            if guard && folder_read::loops(fs, &root, &r) {
                return Got::Loops;
            }
            Got::Listed(folder_read::sorted(fs, &root.join(&r)))
        });
    }

    /// Stop waiting for folder `rel` to open.
    pub(super) fn stop_loading(&mut self, rel: &str) {
        self.disk.waiting.retain(|(_, w)| !matches!(w, What::Open { rel: r, .. } if r == rel));
    }

    /// Put the entries of folder `rel` below it (not when its row is gone).
    pub(super) fn fill(&mut self, rel: &str, children: Vec<(String, Kind)>) {
        let at = match self.entries.iter().position(|x| x.path == rel) {
            Some(i) => i + 1,
            None if rel.is_empty() => 0,
            None => return,
        };
        self.loaded.insert(rel.to_string());
        let mut items = Vec::new();
        for (name, kind) in children {
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if kind == Kind::Unknown {
                self.disk.unknown.insert(path.clone());
            }
            items.push(Entry { path, folder: kind == Kind::Folder, unreadable: false });
        }
        self.entries.splice(at..at, items);
    }

    /// Folder `rel` could not be listed (`slow`: it did not answer in time): marked so, never
    /// shown as empty.
    pub(super) fn mark_unreadable(&mut self, rel: &str, slow: bool) {
        if slow {
            self.disk.slow.insert(rel.to_string());
        } else {
            self.loaded.insert(rel.to_string());
        }
        if let Some(entry) = self.entries.iter_mut().find(|x| x.path == rel) {
            entry.unreadable = true;
        } else {
            // The folder shown: an entry of its own (no row) keeps it marked whatever is
            // pressed, so it never reads as empty.
            self.entries.push(Entry { path: String::new(), folder: true, unreadable: true });
        }
    }

    /// Show the first of `dirs` that is a folder (the last one whatever it is), its entry
    /// selected, nothing open. It is read on a worker; until it answers, the first is shown,
    /// being read.
    pub(super) fn show_first(&mut self, dirs: Vec<(PathBuf, Option<String>)>) {
        let Some((first, _)) = dirs.first() else { return };
        self.root = first.clone();
        self.entries.clear();
        self.loaded = BTreeSet::new();
        self.open.clear();
        self.error = None;
        self.scroll = 0;
        self.detached = false;
        self.row = TreeRow::Top;
        self.disk.waiting.clear();
        self.disk.slow.clear();
        self.disk.unknown.clear();
        let tried = Arc::new(AtomicUsize::new(0));
        let what = What::Show { dirs: dirs.iter().map(|(d, _)| d.clone()).collect(), tried: tried.clone() };
        self.ask(what, first.clone(), move |fs| {
            let last = dirs.len() - 1;
            let i = dirs
                .iter()
                .enumerate()
                .position(|(i, (d, _))| {
                    tried.store(i, Ordering::SeqCst);
                    i == last || fs.is_dir(d)
                })
                .unwrap_or(last);
            let (root, select) = dirs.into_iter().nth(i).unwrap_or_default();
            let items = folder_read::sorted(fs, &root);
            Got::Shown { root, select, items }
        });
    }

    /// Show folder `dir` (its entries listed, nothing open), with `select` selected when it is
    /// there.
    pub(super) fn show_dir(&mut self, dir: PathBuf, select: Option<&str>) {
        self.show_first(vec![(dir, select.map(str::to_string))]);
    }

    /// The folder shown was read.
    fn shown(&mut self, root: PathBuf, select: Option<String>, items: Vec<(String, Kind)>) {
        self.root = root;
        self.fill("", items);
        self.export_reveal(select.as_deref());
        self.row = select
            .as_deref()
            .and_then(|n| self.entries.iter().find(|e| e.path == n))
            .map(|e| if e.folder { TreeRow::Folder(e.path.clone()) } else { TreeRow::File(e.path.clone()) })
            .unwrap_or(TreeRow::Top);
    }

    /// Go up to the parent of the folder shown, the folder left selected.
    pub(super) fn up(&mut self) {
        let Some(parent) = self.root.parent().map(Path::to_path_buf) else { return };
        let name = self.root.file_name().map(|n| n.to_string_lossy().to_string());
        self.show_dir(parent, name.as_deref());
    }
}

impl App {
    /// `Ctrl+O` or the `[…]` button on the SSH key file field: open the picker.
    pub(super) fn open_key_picker(&mut self) {
        let home = self.home();
        let Some(f) = self.overlays.form() else { return };
        let current = f.ssh_key.text().trim().to_string();
        let current = (!current.is_empty()).then(|| expand(&current, home.as_deref()));
        // The field's folder, else the folder of its file, else ~/.ssh, else home (asked on
        // the worker, in this order).
        let (mut dirs, select) = start_dirs(current.as_deref());
        for d in [home.as_ref().map(|h| h.join(".ssh")), home.clone()].into_iter().flatten() {
            dirs.push((d, select.clone()));
        }
        dirs.push((PathBuf::from(std::path::MAIN_SEPARATOR_STR), select));
        let disk = Disk::new(self.folder_reader());
        let Some(f) = self.overlays.form_mut() else { return };
        f.focus_field(profiles::Field::SshKeyFile);
        let mut t = ScriptTree {
            mode: TreeMode::KeyFile,
            entries: Vec::new(),
            open: BTreeSet::new(),
            row: TreeRow::Top,
            focus: TreeFocus::Name,
            input: TextInput::default(),
            error: None,
            scroll: 0,
            detached: false,
            list: Rect::default(),
            overwrite: None,
            root: PathBuf::new(),
            loaded: BTreeSet::new(),
            moving: None,
            disk,
        };
        t.show_first(dirs);
        self.overlays.push(Overlay::ScriptTree(t));
    }

    /// `Enter` in the picker: a typed path is taken as it is; a file is picked; a folder opens
    /// or closes, `../` goes up.
    pub(super) fn key_picker_enter(&mut self) {
        let home = self.home();
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let typed = t.input.text().trim().to_string();
        let path = if is_path(&typed) {
            expand(&typed, home.as_deref())
        } else {
            match t.row.clone() {
                TreeRow::File(p) => t.root.join(p),
                TreeRow::Folder(_) => return t.set_open(None),
                TreeRow::Up => return t.up(),
                TreeRow::Top => return,
            }
        };
        self.overlays.close(OverlayKind::ScriptTree);
        let text = if is_path(&typed) { typed } else { shown(&path, home.as_deref()) };
        let note = key_note(&path, &text);
        if let Some(f) = self.overlays.form_mut() {
            f.ssh_key.set(&text);
            f.focus_field(profiles::Field::SshKeyFile);
            f.ssh_key_note = note;
        }
    }
}

/// Where a picker opened on `current` (the path its field names) starts: `current` when it is
/// a folder, else its folder with its name selected; and what is selected in the folders tried
/// after these.
pub(super) fn start_dirs(current: Option<&Path>) -> (Vec<(PathBuf, Option<String>)>, Option<String>) {
    let Some(p) = current else { return (Vec::new(), None) };
    let mut dirs = vec![(p.to_path_buf(), None)];
    let name = p.file_name().map(|n| n.to_string_lossy().to_string());
    if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        dirs.push((d.to_path_buf(), name.clone()));
    }
    (dirs, name)
}

impl App {
    /// A read of the picker answered (`what` it was for, still waited for).
    pub(super) fn tree_answer(&mut self, what: What, got: Result<Got, NoAnswer>) {
        let home = self.home();
        let other = self.reason_text(&Label::FaultOther.into());
        // Asked for the selection or the text as they were: dropped once either changed.
        let Some(t) = self.overlays.script_tree() else { return };
        let moved = match &what {
            What::SaveHere { dir, .. } | What::NewFolder { dir, .. } => t.export_dir() != *dir,
            What::Typed { text, .. } => t.input.text().trim() != text,
            What::Show { .. } | What::Open { .. } | What::Create { .. } => false,
        };
        if moved {
            return;
        }
        let reason = |app: &App, f: &datarig_core::fault::Fault| app.reason_text(&crate::app::fault_reason(f));
        let error = match (&what, got) {
            (What::Show { .. }, Ok(Got::Shown { root, select, items })) => {
                let Some(t) = self.overlays.script_tree_mut() else { return };
                match items {
                    Ok(items) => {
                        t.shown(root, select, items);
                        return;
                    }
                    Err(f) => {
                        t.root = root;
                        t.mark_unreadable("", false);
                        let path = shown_dir(&t.root, home.as_deref());
                        Some(Msg::FolderUnreadable { path, error: reason(self, &f) })
                    }
                }
            }
            (What::Open { rel, select }, Ok(Got::Listed(Ok(items)))) => {
                let Some(t) = self.overlays.script_tree_mut() else { return };
                // Its row went meanwhile (listed again): nothing to open.
                if !rel.is_empty() && !t.entries.iter().any(|x| x.path == *rel) {
                    return;
                }
                t.fill(rel, items);
                t.open.insert(rel.clone());
                if let Some(s) = select {
                    t.row = TreeRow::Folder(s.clone());
                }
                return;
            }
            (What::Open { rel, .. }, Ok(Got::Listed(Err(f)))) => {
                let Some(t) = self.overlays.script_tree_mut() else { return };
                t.mark_unreadable(rel, false);
                let path = shown(&t.root.join(rel), home.as_deref());
                Some(Msg::FolderUnreadable { path, error: reason(self, &f) })
            }
            (What::Open { rel, .. }, Ok(Got::Loops)) => Some(Msg::ExportPathLoop { name: rel.clone() }),
            (What::SaveHere { dir, .. }, Ok(Got::Readable { result: Ok(()), typed_dir })) => {
                return self.export_path_save_into(dir.clone(), typed_dir);
            }
            (What::NewFolder { parent, .. }, Ok(Got::Readable { result: Ok(()), .. })) => {
                return self.export_path_name_folder(parent.clone());
            }
            (What::SaveHere { dir, .. } | What::NewFolder { dir, .. }, Ok(Got::Readable { result: Err(_), .. })) => {
                Some(Msg::ExportPathUnreadable { path: shown(dir, home.as_deref()) })
            }
            (What::Typed { path, .. }, Ok(Got::IsDir(dir))) => return self.export_path_typed_is(path.clone(), dir),
            (What::Create { parent, name, .. }, Ok(Got::Made(made))) => {
                return self.export_folder_made(parent.clone(), name.clone(), made);
            }
            (What::Create { .. }, Err(no)) => {
                // Whether it was made is not known: the prompt closes and the picker says why.
                if let What::Create { parent, name, .. } = &what
                    && self.export_folder_prompt(parent, name).is_some()
                {
                    self.overlays.close(OverlayKind::NameInput);
                }
                let Some(t) = self.overlays.script_tree_mut() else { return };
                let dir = what_dir(&what, t);
                t.error = Some(no_answer(no, shown(&dir, home.as_deref()), other));
                return;
            }
            (_, Err(no)) => {
                let Some(t) = self.overlays.script_tree_mut() else { return };
                let dir = match &what {
                    What::Show { dirs, tried } => {
                        if let Some(d) = dirs.get(tried.load(Ordering::SeqCst)) {
                            t.root = d.clone();
                        }
                        t.mark_unreadable("", no == NoAnswer::TooSlow);
                        t.root.clone()
                    }
                    What::Open { rel, .. } => {
                        t.mark_unreadable(rel, no == NoAnswer::TooSlow);
                        t.root.join(rel)
                    }
                    What::SaveHere { .. } | What::NewFolder { .. } | What::Create { .. } | What::Typed { .. } => {
                        what_dir(&what, t)
                    }
                };
                Some(no_answer(no, shown(&dir, home.as_deref()), other))
            }
            // An answer of another kind than asked for: nothing is learned.
            _ => None,
        };
        if let Some(t) = self.overlays.script_tree_mut() {
            t.error = error;
        }
    }
}

/// Where read `what` was made, on disk (`t`: the picker).
fn what_dir(what: &What, t: &ScriptTree) -> PathBuf {
    match what {
        What::Show { .. } => t.root.clone(),
        What::Open { rel, .. } => t.root.join(rel),
        What::SaveHere { dir, .. } | What::NewFolder { dir, .. } | What::Create { dir, .. } => dir.clone(),
        What::Typed { path, .. } => path.clone(),
    }
}

/// What a read of `path` that gave no answer says (`other`: the reason of one that failed
/// inside).
fn no_answer(no: NoAnswer, path: String, other: String) -> Msg {
    match no {
        NoAnswer::TooSlow => Msg::FolderTooSlow { path },
        NoAnswer::Broke => Msg::FolderUnreadable { path, error: other },
    }
}

/// What to say below the key file field about the file at `path` (`text`: as the field shows
/// it): a file others may read (the tunnel refuses it), a PuTTY key, a path that is no file.
/// Only its kind and mode are asked for; the file is not opened.
pub fn key_note(path: &Path, text: &str) -> Option<Msg> {
    let meta = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        _ => return Some(Msg::FormSshKeyNotFile { path: text.to_string() }),
    };
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ppk")) {
        return Some(Msg::FormSshKeyPpk { path: text.to_string() });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(Msg::FormSshKeyOpen { path: text.to_string(), mode: format!("{:03o}", mode & 0o777) });
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    None
}

#[cfg(test)]
mod tests;
