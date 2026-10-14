//! The SSH key file picker of the profile form: the tree dialog of the saved
//! queries ([`ScriptTree`] in [`TreeMode::KeyFile`]) over a folder of the file system. It opens
//! at the folder of the field's file, else `~/.ssh` when it exists, else the home folder; `../`
//! (or `←` on the top row) goes up. Hidden files are listed (keys live in `~/.ssh`); likely
//! private keys stand out and public keys, `known_hosts`, `config` and `authorized_keys` are
//! dimmed. Typing filters the listed names; a text with `/` or a leading `~` is a path, which
//! `Enter` takes as it is (`~` is the home folder).
//!
//! Browsing only lists folders and asks for a file's kind (`stat`); no file is opened, so no
//! key is read. Picking one looks at its mode and name only: a key that others may read (the
//! tunnel refuses it) and a PuTTY key (`.ppk`) are said so below the field, and the text is
//! kept either way.

use super::script_tree::{ScriptTree, TreeFocus, TreeMode, TreeRow};
use super::*;
use datarig_core::scripts::Entry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
fn shown(path: &Path, home: Option<&Path>) -> String {
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

/// The entries of folder `dir`: folders first, then files, each by name (case folded). Only
/// the folder is read, and the kind of a link is asked for (`stat`); no file is opened.
fn list(dir: &Path) -> std::io::Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let folder = match e.file_type() {
            Ok(t) if t.is_symlink() => std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()),
            Ok(t) => t.is_dir(),
            Err(_) => false,
        };
        out.push((name, folder));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    Ok(out)
}

impl ScriptTree {
    /// Key file mode: list folder `rel` (relative to the root, `""` for the root) and put its
    /// entries below it. A folder that cannot be listed is marked so.
    pub(super) fn load(&mut self, rel: &str) {
        let dir = if rel.is_empty() { self.root.clone() } else { self.root.join(rel) };
        self.loaded.insert(rel.to_string());
        let children = match list(&dir) {
            Ok(c) => c,
            Err(e) => {
                if let Some(entry) = self.entries.iter_mut().find(|x| x.path == rel) {
                    entry.unreadable = true;
                } else {
                    self.error = Some(crate::app::fault_reason(&datarig_core::fault::Fault::io_at(&e, &dir)));
                }
                return;
            }
        };
        let at = match self.entries.iter().position(|x| x.path == rel) {
            Some(i) => i + 1,
            None => 0,
        };
        let items: Vec<Entry> = children
            .into_iter()
            .map(|(name, folder)| Entry {
                path: if rel.is_empty() { name } else { format!("{rel}/{name}") },
                folder,
                unreadable: false,
            })
            .collect();
        self.entries.splice(at..at, items);
    }

    /// Key file mode: show folder `dir` (its entries listed, nothing open), with `select`
    /// selected when it is there.
    pub(super) fn show_dir(&mut self, dir: PathBuf, select: Option<&str>) {
        self.root = dir;
        self.entries.clear();
        self.loaded = BTreeSet::new();
        self.open.clear();
        self.error = None;
        self.scroll = 0;
        self.detached = false;
        self.load("");
        self.row = select
            .and_then(|n| self.entries.iter().find(|e| e.path == n))
            .map(|e| if e.folder { TreeRow::Folder(e.path.clone()) } else { TreeRow::File(e.path.clone()) })
            .unwrap_or(TreeRow::Top);
    }

    /// Key file mode: go up to the parent of the folder shown, the folder left selected.
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
        let Some(f) = self.overlays.form_mut() else { return };
        f.focus_field(profiles::Field::SshKeyFile);
        let current = f.ssh_key.text().trim().to_string();
        let current = (!current.is_empty()).then(|| expand(&current, home.as_deref()));
        let is_dir = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_dir());
        // The folder of the field's file (or the field's folder), else ~/.ssh, else home.
        let (dir, select) = match &current {
            Some(p) if is_dir(p) => (Some(p.clone()), None),
            Some(p) => (
                p.parent().filter(|d| !d.as_os_str().is_empty() && is_dir(d)).map(Path::to_path_buf),
                p.file_name().map(|n| n.to_string_lossy().to_string()),
            ),
            None => (None, None),
        };
        let dir = dir
            .or_else(|| home.as_ref().map(|h| h.join(".ssh")).filter(|d| is_dir(d)))
            .or_else(|| home.clone().filter(|h| is_dir(h)))
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR));
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
        };
        t.show_dir(dir, select.as_deref());
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
