//! The folder tree of the saved queries for "save as" and "open" : the folders and `.sql` files of the saved-queries folder as a tree, with a name
//! field (save) or a filter line (open) below it.
//!
//! Save: `↑`/`↓` move (from either part), `→`/`←` open and close a folder (tree), `n` a new
//! folder under the selected one (tree), `Tab` between the tree and the name, `Enter` saves into
//! the selected folder (a selected file stands for its folder and gives its name). A name with
//! `/` still makes folders, below the selected folder. A name that exists asks before it is
//! replaced (Cancel is the default); one open in another tab is refused. Open: typing filters,
//! `Enter` opens a file or opens a folder. A folder that cannot be read says so and is never
//! taken for an empty one; nothing is saved into it.
//!
//! The same dialog picks the SSH key file of the profile form
//! ([`TreeMode::KeyFile`], see `key_picker`): a folder of the file system instead of the
//! saved-queries folder, listed a folder at a time as folders open, with `../` above it.

use super::chooser::{NameInput, NamePurpose};
use super::*;
use crate::widgets::text_input::InputResult;
use datarig_core::scripts::{self, Entry, name::NameError, stem_path};
use std::collections::BTreeSet;

/// What the dialog is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeMode {
    /// Save tab `tab` as a saved query (`then_close`: `:wq`).
    Save { tab: TabId, then_close: bool },
    /// Open a saved query.
    Open,
    /// Pick the SSH key file of the profile form.
    KeyFile,
}

/// Which part has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeFocus {
    Tree,
    /// The name field (save) or the filter line (open).
    Name,
}

/// A row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    /// The parent of the folder shown (key file mode).
    Up,
    /// The saved-queries folder itself (key file mode: the folder shown).
    Top,
    Folder(String),
    File(String),
}

pub struct ScriptTree {
    pub mode: TreeMode,
    /// The folders and scripts, as listed when the dialog opened (or a folder was made).
    pub entries: Vec<Entry>,
    /// Open folders.
    pub open: BTreeSet<String>,
    /// The selected row.
    pub row: TreeRow,
    pub focus: TreeFocus,
    /// The name (save) or the filter (open).
    pub input: TextInput,
    /// Why the last `Enter` did nothing; typing clears it.
    pub error: Option<Msg>,
    /// First row shown, and whether the wheel moved it away from the selection.
    pub scroll: usize,
    pub detached: bool,
    /// Where the rows were drawn (mouse), kept by the renderer.
    pub list: Rect,
    /// The saved query to replace once the user says so.
    pub overwrite: Option<String>,
    /// Key file mode: the folder the top row stands for, and the folders listed so far
    /// (relative to it, `""` for itself).
    pub root: std::path::PathBuf,
    pub loaded: BTreeSet<String>,
}

impl ScriptTree {
    fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// The rows shown, with their depth: the top, then what the open folders hold (open mode
    /// with a filter: every file whose path matches, and the folders on the way).
    pub fn rows(&self) -> Vec<(TreeRow, usize)> {
        let mut out = Vec::new();
        if self.mode == TreeMode::KeyFile && self.root.parent().is_some() {
            out.push((TreeRow::Up, 0));
        }
        out.push((TreeRow::Top, 0));
        let q = scripts::name::fold(self.input.text().trim());
        let filtering = self.filtering();
        let key_file = self.mode == TreeMode::KeyFile;
        let matches = |p: &str| {
            let p = if key_file { p.rsplit('/').next().unwrap_or(p) } else { stem_path(p) };
            scripts::name::fold(p).contains(&q)
        };
        let open = |path: &str| {
            let mut dir = scripts::parent(path);
            while let Some(d) = dir {
                if !self.open.contains(d) {
                    return false;
                }
                dir = scripts::parent(d);
            }
            true
        };
        for e in &self.entries {
            let shown = if filtering {
                if e.folder {
                    self.entries.iter().any(|x| !x.folder && scripts::is_within(&x.path, &e.path) && matches(&x.path))
                } else {
                    matches(&e.path)
                }
            } else {
                open(&e.path)
            };
            if shown {
                let row = if e.folder { TreeRow::Folder(e.path.clone()) } else { TreeRow::File(e.path.clone()) };
                out.push((row, e.depth() + 1));
            }
        }
        out
    }

    /// The text below the tree filters the rows: open mode, and key file mode unless it is a
    /// path (`/` or `~` in it).
    pub fn filtering(&self) -> bool {
        let q = self.input.text().trim();
        !q.is_empty()
            && match self.mode {
                TreeMode::Open => true,
                TreeMode::KeyFile => !super::key_picker::is_path(q),
                TreeMode::Save { .. } => false,
            }
    }

    /// Index of the selected row in [`ScriptTree::rows`] (the first when it is not shown).
    pub fn selected(&self) -> usize {
        self.rows().iter().position(|(r, _)| *r == self.row).unwrap_or(0)
    }

    /// The folder the selection stands for (`None`: the top).
    pub fn folder(&self) -> Option<String> {
        match &self.row {
            TreeRow::Up | TreeRow::Top => None,
            TreeRow::Folder(p) => Some(p.clone()),
            TreeRow::File(p) => scripts::parent(p).map(str::to_string),
        }
    }

    /// Whether a folder row is open (`None`: not a folder).
    pub fn is_open(&self, row: &TreeRow) -> Option<bool> {
        match row {
            TreeRow::Top => Some(true),
            TreeRow::Folder(p) => Some(self.open.contains(p) || self.filtering()),
            TreeRow::Up | TreeRow::File(_) => None,
        }
    }

    /// A folder whose contents could not be listed.
    pub fn unreadable(&self, row: &TreeRow) -> bool {
        matches!(row, TreeRow::Folder(p) if self.entry(p).is_some_and(|e| e.unreadable))
    }

    /// Select row `i`; in save mode a file gives the name field its name.
    fn select(&mut self, i: usize) {
        let rows = self.rows();
        let Some((row, _)) = rows.get(i.min(rows.len().saturating_sub(1))).cloned() else { return };
        if let (TreeMode::Save { .. }, TreeRow::File(p)) = (self.mode, &row) {
            self.input.set(scripts::display_name(p, false));
        }
        self.row = row;
        self.detached = false;
        self.error = None;
    }

    fn step(&mut self, d: isize) {
        let n = self.rows().len() as isize;
        let i = (self.selected() as isize + d).clamp(0, n - 1);
        self.select(i as usize);
    }

    /// Open (`Some(true)`), close or toggle the selected folder.
    pub(super) fn set_open(&mut self, want: Option<bool>) {
        let TreeRow::Folder(p) = self.row.clone() else { return };
        if self.unreadable(&self.row) {
            return;
        }
        let open = self.open.contains(&p);
        let want = want.unwrap_or(!open);
        if want && self.mode == TreeMode::KeyFile && !self.loaded.contains(&p) {
            self.load(&p);
            if self.unreadable(&self.row) {
                return;
            }
        }
        match want {
            true => self.open.insert(p),
            false => self.open.remove(&p),
        };
    }
}

impl App {
    /// Open the tree dialog for `mode`. The selection starts on the tab's saved query (its name
    /// in the field for "save as"), else on the folder saved into last this run, else the top;
    /// the folders on the way are open.
    pub(super) fn open_script_tree(&mut self, mode: TreeMode) {
        if mode == TreeMode::KeyFile {
            return self.open_key_picker();
        }
        if !self.scripts_available() {
            return self.flash(Notice::new(Label::ScriptsNoDataDir, Level::Warning));
        }
        self.refresh_scripts();
        let entries = self.scripts.as_ref().map(|s| s.list()).unwrap_or_default();
        let current = match mode {
            TreeMode::Save { tab, .. } => self.tabs.get(tab).and_then(|t| t.script().map(str::to_string)),
            TreeMode::Open | TreeMode::KeyFile => self.tab().script().map(str::to_string),
        };
        let (row, name) = match (&current, mode) {
            (Some(p), TreeMode::Save { .. }) => (TreeRow::File(p.clone()), scripts::display_name(p, false).to_string()),
            (Some(p), TreeMode::Open | TreeMode::KeyFile) => (TreeRow::File(p.clone()), String::new()),
            (None, _) => (self.last_save_folder.clone().map_or(TreeRow::Top, TreeRow::Folder), String::new()),
        };
        // The folders on the way to the selection are open.
        let mut open = BTreeSet::new();
        let path = match &row {
            TreeRow::File(p) => scripts::parent(p).map(str::to_string),
            TreeRow::Folder(p) => Some(p.clone()),
            TreeRow::Up | TreeRow::Top => None,
        };
        let mut dir = path.as_deref();
        while let Some(d) = dir {
            open.insert(d.to_string());
            dir = scripts::parent(d);
        }
        let row = if matches!(&row, TreeRow::Folder(p) if !entries.iter().any(|e| e.folder && e.path == *p)) {
            TreeRow::Top
        } else {
            row
        };
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::ScriptTree(ScriptTree {
            mode,
            entries,
            open,
            row,
            focus: TreeFocus::Name,
            input: TextInput::new(&name),
            error: None,
            scroll: 0,
            detached: false,
            list: Rect::default(),
            overwrite: None,
            root: Default::default(),
            loaded: BTreeSet::new(),
        }));
    }

    /// Keys of the tree dialog (`overlay.script_tree`, `overlay.script_tree.name`).
    pub(super) fn script_tree_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let tree = t.focus == TreeFocus::Tree;
        let key_file = t.mode == TreeMode::KeyFile;
        match key.code {
            KeyCode::Esc => self.overlays.close(OverlayKind::ScriptTree),
            // Key file mode: `←` on the folder shown goes up to its parent.
            KeyCode::Left | KeyCode::Char('h') if tree && key_file && matches!(t.row, TreeRow::Up | TreeRow::Top) => {
                t.up();
            }
            KeyCode::Right | KeyCode::Char('l') if tree && key_file && t.row == TreeRow::Up => t.up(),
            KeyCode::Enter if !repeat => self.script_tree_enter(),
            KeyCode::Enter => {}
            KeyCode::Up => t.step(-1),
            KeyCode::Down => t.step(1),
            KeyCode::Tab | KeyCode::BackTab => {
                t.focus = if tree { TreeFocus::Name } else { TreeFocus::Tree };
            }
            KeyCode::Char('k') if tree => t.step(-1),
            KeyCode::Char('j') if tree => t.step(1),
            KeyCode::Right | KeyCode::Char('l') if tree => t.set_open(Some(true)),
            KeyCode::Left | KeyCode::Char('h') if tree => {
                if t.is_open(&t.row) == Some(true) && t.row != TreeRow::Top {
                    t.set_open(Some(false));
                } else if let Some(parent) = t.folder().filter(|_| matches!(t.row, TreeRow::File(_))).or_else(|| {
                    if let TreeRow::Folder(p) = &t.row { scripts::parent(p).map(str::to_string) } else { None }
                }) {
                    t.row = TreeRow::Folder(parent);
                } else {
                    t.row = TreeRow::Top;
                }
            }
            KeyCode::Char('n') if tree && !repeat && matches!(t.mode, TreeMode::Save { .. }) => {
                let parent = t.folder();
                if t.row != TreeRow::Top && t.unreadable(&TreeRow::Folder(parent.clone().unwrap_or_default())) {
                    t.error = Some(Msg::Label(Label::ValidateScriptUnreadable));
                    return;
                }
                let title = match &parent {
                    Some(p) => Msg::NameNewFolderIn { parent: p.clone() },
                    None => Msg::Label(Label::NameNewFolder),
                };
                self.overlays.push(Overlay::NameInput(NameInput {
                    title,
                    input: TextInput::default(),
                    error: None,
                    purpose: NamePurpose::NewScriptFolder { parent },
                }));
            }
            _ if tree => {}
            _ => {
                if t.input.handle_key(&key) == InputResult::Changed {
                    t.error = None;
                    // Open: the first file that matches the filter.
                    if t.filtering() {
                        let rows = t.rows();
                        if let Some(i) = rows.iter().position(|(r, _)| matches!(r, TreeRow::File(_))) {
                            t.row = rows[i].0.clone();
                        }
                    }
                }
            }
        }
    }

    /// `Enter`: save into the selected folder, or open the selected file (a folder opens).
    fn script_tree_enter(&mut self) {
        let Some(t) = self.overlays.script_tree() else { return };
        let (mode, row) = (t.mode, t.row.clone());
        match mode {
            TreeMode::KeyFile => self.key_picker_enter(),
            TreeMode::Open => match row {
                TreeRow::File(p) => {
                    self.overlays.close(OverlayKind::ScriptTree);
                    self.open_script(&p);
                }
                _ => {
                    if let Some(t) = self.overlays.script_tree_mut() {
                        t.set_open(None);
                    }
                }
            },
            TreeMode::Save { tab, then_close } => {
                let folder = t.folder();
                let typed = t.input.text().trim().to_string();
                if t.unreadable(&TreeRow::Folder(folder.clone().unwrap_or_default())) && folder.is_some() {
                    return self.tree_error(Msg::Label(Label::ValidateScriptUnreadable));
                }
                if typed.is_empty() {
                    return self.tree_error(Msg::Label(Label::ValidateScriptEmpty));
                }
                let input = match &folder {
                    Some(f) => format!("{f}/{typed}"),
                    None => typed,
                };
                let Some(store) = self.scripts.as_ref() else { return };
                let path = match store.resolve_new(&input, None) {
                    Ok(p) => p,
                    Err(NameError::Exists(e)) if store.stamp(&e).is_some() => {
                        return self.ask_overwrite(tab, e);
                    }
                    Err(e) => return self.tree_error(Msg::Label(super::script_ops::name_error(store, &e))),
                };
                self.save_into(tab, &path, false, then_close);
            }
        }
    }

    fn tree_error(&mut self, m: Msg) {
        if let Some(t) = self.overlays.script_tree_mut() {
            t.error = Some(m);
        }
    }

    /// The name is taken by saved query `path`: the tab's own file saves as usual; one open in
    /// another tab is refused (its tab would not know); anything else asks first, Cancel
    /// focused.
    fn ask_overwrite(&mut self, tab: TabId, path: String) {
        let fold = |p: &str| scripts::name::fold(p);
        let own = self.tabs.get(tab).and_then(|t| t.script()).is_some_and(|p| fold(p) == fold(&path));
        let then_close =
            matches!(self.overlays.script_tree().map(|t| t.mode), Some(TreeMode::Save { then_close: true, .. }));
        if own {
            self.overlays.close(OverlayKind::ScriptTree);
            self.save_current_in(tab);
            if then_close {
                self.close_or_quit();
            }
            return;
        }
        if self.tabs.find_script(&path).is_some() {
            let name = stem_path(&path).to_string();
            return self.tree_error(Msg::ScriptsOpenElsewhere { name });
        }
        if let Some(t) = self.overlays.script_tree_mut() {
            t.overwrite = Some(path.clone());
        }
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ScriptsOverwriteTitle,
            text: Msg::ScriptsOverwrite { name: stem_path(&path).to_string() },
            details: Vec::new(),
            keys: Label::ScriptsOverwriteKeys,
            action: ConfirmAction::OverwriteScript,
            folder: None,
            path: Some(path),
        }));
    }

    /// "Replace": write the tab's text over the saved query the tree dialog asked about.
    pub(super) fn overwrite_confirmed(&mut self) {
        let Some(t) = self.overlays.script_tree() else { return };
        let (Some(path), TreeMode::Save { tab, then_close }) = (t.overwrite.clone(), t.mode) else { return };
        self.save_into(tab, &path, true, then_close);
    }

    /// Save tab `tab` as saved query `path` (new, or `replace` an existing one), then close the
    /// dialog; a failure stays in the dialog.
    fn save_into(&mut self, tab: TabId, path: &str, replace: bool, then_close: bool) {
        match self.save_as_path(tab, path, replace) {
            Ok(()) => {
                self.overlays.close(OverlayKind::ScriptTree);
                self.last_save_folder = scripts::parent(path).map(str::to_string);
                if then_close && self.tabs.get(tab).is_some_and(|t| t.script().is_some()) {
                    self.switch_tab(|m| m.position(tab).is_some_and(|i| m.activate(i)));
                    self.close_or_quit();
                }
            }
            Err(l) => {
                if let Some(t) = self.overlays.script_tree_mut() {
                    t.overwrite = None;
                    t.error = Some(Msg::Label(l));
                }
            }
        }
    }

    /// A folder was made in the tree dialog: list again and select it.
    pub(super) fn tree_folder_made(&mut self, path: &str) {
        self.refresh_scripts();
        let entries = self.scripts.as_ref().map(|s| s.list()).unwrap_or_default();
        if let Some(t) = self.overlays.script_tree_mut() {
            t.entries = entries;
            let mut dir = scripts::parent(path);
            while let Some(d) = dir {
                t.open.insert(d.to_string());
                dir = scripts::parent(d);
            }
            t.row = TreeRow::Folder(path.to_string());
            t.focus = TreeFocus::Name;
            t.error = None;
        }
    }

    /// The mouse on the tree dialog: a click selects a row (on a folder's arrow, or again on
    /// the selected folder, it opens or closes it; again on a file of the open dialog, it opens
    /// it); the wheel scrolls the rows.
    pub(super) fn script_tree_mouse(&mut self, m: MouseEvent) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let (x, y) = (m.column, m.row);
        let list = t.list;
        let inside = x >= list.x && x < list.x + list.width && y >= list.y && y < list.y + list.height;
        match m.kind {
            MouseEventKind::ScrollDown => {
                t.scroll += WHEEL_STEP as usize;
                t.detached = true;
            }
            MouseEventKind::ScrollUp => {
                t.scroll = t.scroll.saturating_sub(WHEEL_STEP as usize);
                t.detached = true;
            }
            MouseEventKind::Down(MouseButton::Left) if inside => {
                let rows = t.rows();
                let i = t.scroll + usize::from(y - list.y);
                let Some((row, depth)) = rows.get(i).cloned() else { return };
                let again = t.row == row;
                t.focus = TreeFocus::Tree;
                let arrow = x.saturating_sub(list.x) as usize / 2 == depth;
                t.select(i);
                match row {
                    TreeRow::Up if again => t.up(),
                    TreeRow::Folder(_) if arrow || again => t.set_open(None),
                    TreeRow::File(_) if again && t.mode == TreeMode::KeyFile => self.key_picker_enter(),
                    TreeRow::File(p) if again && t.mode == TreeMode::Open => {
                        self.overlays.close(OverlayKind::ScriptTree);
                        self.open_script(&p);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
