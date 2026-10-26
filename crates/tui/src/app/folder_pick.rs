//! The folder picker: the tree dialog ([`ScriptTree`] in [`TreeMode::Folder`]) over the
//! profile folders (`m` on a profile, the Folder field of the profile form) or over the folders
//! of the saved queries (`m` on a saved query or folder; a moved folder and what it holds are
//! no rows). Only folders are rows, below "(top level)"; the current folder is selected, the
//! folders on the way to it open.
//!
//! Tree: `↑`/`↓` (`j`/`k`) move, `→`/`←` (`l`/`h`) open and close a folder (`←` on a closed
//! one goes to its parent), `Enter` picks the selected folder, `/` or `Tab` puts the keyboard
//! on the filter, `n` names a new folder inside the selected one, `Esc`/`q` cancel. Filter:
//! typing keeps the folders whose name holds the text (ignoring case) and the folders on the
//! way to them; the selection goes to the first match unless it is on one. `↑`/`↓` move the
//! selection (the keyboard stays on the filter), `Enter` or `Tab` go back to the tree, `Esc`
//! clears the filter and goes back. A new folder is refused when a folder beside it has its
//! name, ignoring case; a folder of the saved queries that cannot be read is not picked.
//!
//! A new profile folder is only a row until it is picked, so browsing never writes the config;
//! a new folder of the saved queries is made on disk at once, as in "save as".

use super::chooser::{NameInput, NamePurpose};
use super::profiles::Field;
use super::script_tree::{ScriptTree, TreeFocus, TreeMode, TreeRow};
use super::*;
use crate::widgets::text_input::InputResult;
use datarig_core::profile::folder::FolderPath;
use datarig_core::scripts::{self, Entry, display_name, is_within};
use std::collections::BTreeSet;

/// What the picked folder is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderPick {
    /// Move this profile there (`m`).
    MoveProfile(ProfileId),
    /// The Folder field of the profile form.
    ProfileForm,
    /// Move the saved query or folder [`ScriptTree::moving`] there (`m`).
    MoveScript,
}

impl FolderPick {
    pub fn title(self) -> Label {
        match self {
            FolderPick::ProfileForm => Label::FormFieldFolder,
            FolderPick::MoveProfile(_) | FolderPick::MoveScript => Label::ChooserMoveTitle,
        }
    }
}

/// Profile folders `paths` (and the folders on the way to them) as rows in tree order: a
/// folder before what it holds, siblings by name ignoring case, as the explorer has them.
fn profile_entries(paths: impl IntoIterator<Item = String>) -> Vec<Entry> {
    let mut all: Vec<String> = paths
        .into_iter()
        .filter_map(|p| FolderPath::parse(&p).ok())
        .flat_map(|p| p.with_ancestors())
        .map(|p| p.to_string())
        .collect();
    // Segment by segment, so `A/x` stays under `A` whatever `a` is beside it.
    let key = |p: &str| p.split('/').map(|s| (s.to_lowercase(), s.to_string())).collect::<Vec<_>>();
    all.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.cmp(b)));
    all.dedup();
    all.into_iter().map(|path| Entry { path, folder: true, unreadable: false }).collect()
}

impl ScriptTree {
    /// With filter `q` (case folded): folder `path` is shown when it or a folder inside it has
    /// a name that holds `q`.
    pub(super) fn folder_shown(&self, path: &str, q: &str) -> bool {
        self.entries.iter().any(|e| is_within(&e.path, path) && folder_matches(&e.path, q))
    }

    /// The saved queries listed again: only the folders, without the moved one.
    pub(super) fn keep_folders(&mut self) {
        let moving = self.moving.clone();
        self.entries.retain(|e| e.folder && !moving.as_deref().is_some_and(|m| is_within(&e.path, m)));
        self.input.set("");
    }

    /// Open the folders on the way to `path`.
    fn open_to(&mut self, path: &str) {
        let mut dir = scripts::parent(path);
        while let Some(d) = dir {
            self.open.insert(d.to_string());
            dir = scripts::parent(d);
        }
    }

    /// The filter changed: the selection goes to the first match unless it is on one (the top
    /// when nothing matches); without a filter the folders on the way to it open.
    fn follow_filter(&mut self) {
        let q = scripts::name::fold(self.input.text().trim());
        if !q.is_empty() && !matches!(&self.row, TreeRow::Folder(p) if folder_matches(p, &q)) {
            let rows = self.rows();
            match rows.iter().position(|(r, _)| matches!(r, TreeRow::Folder(p) if folder_matches(p, &q))) {
                Some(i) => self.select(i),
                None => self.row = TreeRow::Top,
            }
        } else if let TreeRow::Folder(p) = self.row.clone() {
            self.open_to(&p);
        }
    }
}

/// The name of folder `path` holds `q` (case folded).
fn folder_matches(path: &str, q: &str) -> bool {
    scripts::name::fold(display_name(path, true)).contains(q)
}

impl App {
    /// The folder picker for `pick` over `entries`, `current` selected; `moving`: the saved
    /// query or folder moved.
    fn open_folder_pick(
        &mut self,
        pick: FolderPick,
        entries: Vec<Entry>,
        current: Option<String>,
        moving: Option<String>,
    ) {
        let mut t = ScriptTree {
            mode: TreeMode::Folder(pick),
            entries,
            open: BTreeSet::new(),
            row: TreeRow::Top,
            focus: TreeFocus::Tree,
            input: TextInput::default(),
            error: None,
            scroll: 0,
            detached: false,
            list: Rect::default(),
            overwrite: None,
            root: Default::default(),
            loaded: BTreeSet::new(),
            moving,
            disk: Default::default(),
        };
        if let Some(c) = current.filter(|c| t.entries.iter().any(|e| e.path == *c)) {
            t.open_to(&c);
            t.row = TreeRow::Folder(c);
        }
        self.overlays.push(Overlay::ScriptTree(t));
    }

    /// `m` on a profile.
    pub(super) fn open_move_profile(&mut self, id: ProfileId) {
        let current = self.profile(id).and_then(|p| p.folder.clone());
        let entries = profile_entries(self.folders.iter().map(ToString::to_string));
        self.open_folder_pick(FolderPick::MoveProfile(id), entries, current, None);
    }

    /// `Enter` (or a click) on the Folder field of the profile form. Its folder is a row even
    /// before the form is saved.
    pub(super) fn open_form_folder_pick(&mut self) {
        let Some(f) = self.overlays.form() else { return };
        let current = f.folder.clone();
        let entries = profile_entries(f.folders.iter().cloned().chain(current.clone()));
        self.open_folder_pick(FolderPick::ProfileForm, entries, current, None);
    }

    /// `m` on a saved query or folder: a folder is not moved into itself.
    pub(super) fn open_move_script(&mut self, path: String) {
        let entries = self.script_list.iter().filter(|e| e.folder && !is_within(&e.path, &path)).cloned().collect();
        let current = scripts::parent(&path).map(str::to_string);
        self.open_folder_pick(FolderPick::MoveScript, entries, current, Some(path));
    }

    /// Keys of the folder picker (`overlay.script_tree`, `overlay.script_tree.name`).
    pub(super) fn folder_pick_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        if t.focus == TreeFocus::Name {
            match key.code {
                KeyCode::Esc => {
                    t.input.set("");
                    t.focus = TreeFocus::Tree;
                    t.follow_filter();
                }
                KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => t.focus = TreeFocus::Tree,
                KeyCode::Up => t.step(-1),
                KeyCode::Down => t.step(1),
                _ => {
                    if t.input.handle_key(&key) == InputResult::Changed {
                        t.error = None;
                        t.follow_filter();
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => t.step(-1),
            KeyCode::Down | KeyCode::Char('j') => t.step(1),
            KeyCode::Right | KeyCode::Char('l') => t.set_open(Some(true)),
            KeyCode::Left | KeyCode::Char('h') => {
                if let TreeRow::Folder(p) = t.row.clone() {
                    if t.open.contains(&p) && !t.filtering() {
                        t.set_open(Some(false));
                    } else {
                        t.row = scripts::parent(&p).map_or(TreeRow::Top, |d| TreeRow::Folder(d.to_string()));
                    }
                }
            }
            _ if repeat => {}
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('/') => t.focus = TreeFocus::Name,
            KeyCode::Esc | KeyCode::Char('q') => self.overlays.close(OverlayKind::ScriptTree),
            KeyCode::Enter => self.folder_picked(),
            KeyCode::Char('n') => self.open_pick_new_folder(),
            _ => {}
        }
    }

    /// `n`: name a new folder inside the selected one. One of the saved queries is not made in
    /// a folder that cannot be read.
    fn open_pick_new_folder(&mut self) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let TreeMode::Folder(pick) = t.mode else { return };
        let parent = t.folder();
        let purpose = match pick {
            FolderPick::MoveScript if t.unreadable(&t.row) => {
                t.error = Some(Msg::Label(Label::ValidateScriptUnreadable));
                return;
            }
            FolderPick::MoveScript => NamePurpose::NewScriptFolder { parent: parent.clone() },
            FolderPick::MoveProfile(_) | FolderPick::ProfileForm => {
                NamePurpose::NewPickFolder { parent: parent.clone() }
            }
        };
        let title = match parent {
            Some(parent) => Msg::NameNewFolderIn { parent },
            None => Msg::Label(Label::NameNewFolder),
        };
        self.overlays.push(Overlay::NameInput(NameInput {
            title,
            input: TextInput::default(),
            error: None,
            purpose,
            buttons: Default::default(),
        }));
    }

    /// A new profile folder `typed` inside `parent`: a row of the picker, selected; nothing is
    /// saved until it is picked.
    pub(super) fn new_pick_folder(&mut self, parent: Option<String>, typed: &str) -> Result<(), Label> {
        let path = match &parent {
            Some(p) => format!("{p}/{typed}"),
            None => typed.to_string(),
        };
        let path = FolderPath::parse(&path).map_err(|e| super::chooser::folder_error(&e))?.to_string();
        let Some(t) = self.overlays.script_tree_mut() else { return Ok(()) };
        let name = scripts::name::fold(display_name(&path, true));
        let parent = scripts::parent(&path);
        if t.entries
            .iter()
            .any(|e| scripts::parent(&e.path) == parent && scripts::name::fold(display_name(&e.path, true)) == name)
        {
            return Err(Label::ValidateFolderExists);
        }
        let paths: Vec<String> = t.entries.drain(..).map(|e| e.path).chain([path.clone()]).collect();
        t.entries = profile_entries(paths);
        t.input.set("");
        t.open_to(&path);
        t.row = TreeRow::Folder(path);
        t.focus = TreeFocus::Tree;
        t.error = None;
        Ok(())
    }

    /// Pick the selected folder (the top: the top level) and close the picker.
    pub(super) fn folder_picked(&mut self) {
        let Some(t) = self.overlays.script_tree() else { return };
        let TreeMode::Folder(pick) = t.mode else { return };
        let (folder, moving) = (t.folder(), t.moving.clone());
        if pick == FolderPick::MoveScript && t.unreadable(&t.row) {
            if let Some(t) = self.overlays.script_tree_mut() {
                t.error = Some(Msg::Label(Label::ValidateScriptUnreadable));
            }
            return;
        }
        self.overlays.close(OverlayKind::ScriptTree);
        match pick {
            FolderPick::MoveProfile(id) => {
                if self.profile(id).is_none() {
                    return;
                }
                // A new folder joins the config with the move: one write.
                if let Some(f) = folder.as_deref().and_then(|f| FolderPath::parse(f).ok()) {
                    self.folders.insert(&f);
                }
                self.move_profile(id, folder);
            }
            FolderPick::ProfileForm => {
                let Some(f) = self.overlays.form_mut() else { return };
                // A new folder is a choice of the form; it is saved with the form.
                for a in folder
                    .as_deref()
                    .and_then(|v| FolderPath::parse(v).ok())
                    .map(|p| p.with_ancestors())
                    .unwrap_or_default()
                {
                    let a = a.to_string();
                    if !f.folders.contains(&a) {
                        f.folders.push(a);
                    }
                }
                f.folders.sort();
                f.set_choice(Field::Folder, folder);
            }
            FolderPick::MoveScript => {
                if let Some(path) = moving {
                    self.move_script(&path, folder);
                }
            }
        }
    }
}
