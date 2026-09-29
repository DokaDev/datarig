//! Saved queries in the app: `Ctrl+S` on a
//! console asks for a name and turns the console into a script; `Ctrl+S` on a script saves it.
//! The explorer's "Saved queries" section opens, renames, moves and deletes them. A script is
//! open in one tab at most: opening it again goes to that tab. A script that changed on disk
//! since it was read is never overwritten without asking.

use super::chooser::{Chooser, ChooserPurpose, NameInput, NamePurpose};
use super::persist::Saved;
use super::*;
use datarig_core::scripts::name::NameError;
use datarig_core::scripts::{self, display_name, is_within, stem_path};

/// The message of a name that cannot be a saved query's (in `store`: a name a folder has says
/// so).
pub(super) fn name_error(store: &scripts::ScriptStore, e: &NameError) -> Label {
    match e {
        NameError::Empty => Label::ValidateScriptEmpty,
        NameError::EmptyPart => Label::ValidateScriptEmptyPart,
        NameError::Dots => Label::ValidateFolderDots,
        NameError::Hidden => Label::ValidateScriptHidden,
        NameError::Edges => Label::ValidateScriptEdges,
        NameError::Char(_) => Label::ValidateScriptChar,
        NameError::Reserved(_) => Label::ValidateScriptReserved,
        NameError::TooLong => Label::ValidateScriptTooLong,
        NameError::Exists(p) => exists_label(store, p),
        NameError::Unreadable(_) => Label::ValidateScriptUnreadable,
    }
}

/// A name taken by `path` of `store`: by a folder or by a saved query.
fn exists_label(store: &scripts::ScriptStore, path: &str) -> Label {
    if store.file(path).is_dir() { Label::ValidateFolderHasName } else { Label::ValidateScriptExists }
}

impl App {
    /// There is a data directory to keep saved queries in.
    pub fn scripts_available(&self) -> bool {
        self.scripts.is_some()
    }

    /// `Ctrl+S`: a console asks for a name (it becomes a saved query); a saved query is
    /// written now.
    pub(super) fn save_current(&mut self) {
        if !self.scripts_available() {
            return self.flash(Notice::new(Label::ScriptsNoDataDir, Level::Warning));
        }
        let id = self.tab().id;
        match self.tab().script().map(str::to_string) {
            None => self.ask_script_name(id, false),
            Some(path) => {
                if self.tab().doc.conflict {
                    return self.script_conflict(id, self.scripts.as_ref().is_some_and(|s| s.stamp(&path).is_none()));
                }
                if self.save_tab(id) == Saved::Ok {
                    self.sync_binding(id);
                    let name = display_name(&path, false).to_string();
                    self.flash(Notice::new(Msg::ScriptsSaved { name }, Level::Success));
                }
            }
        }
    }

    /// "Save as" for tab `tab` (`then_close`: `:wq`): the folder tree with a name field (step
    /// 2.7.1).
    pub(super) fn ask_script_name(&mut self, tab: TabId, then_close: bool) {
        self.open_script_tree(super::script_tree::TreeMode::Save { tab, then_close });
    }

    /// Save tab `tab` as the saved query named `input`: a new file with the tab's text, bound
    /// to the tab's profile; the tab shows that file from now on (a console's own file goes).
    pub(super) fn save_as(&mut self, tab: TabId, input: &str) -> Result<(), Label> {
        let Some(store) = self.scripts.as_ref() else { return Err(Label::ScriptsNoDataDir) };
        let path = store.resolve_new(input, None).map_err(|e| name_error(store, &e))?;
        self.save_as_path(tab, &path, false)
    }

    /// Save tab `tab` as saved query `path` (checked already): a new file, or with `replace`
    /// the existing one written over (the user said so). The tab shows that file from now on.
    pub(super) fn save_as_path(&mut self, tab: TabId, path: &str, replace: bool) -> Result<(), Label> {
        let Some(store) = self.scripts.as_ref() else { return Err(Label::ScriptsNoDataDir) };
        let path = path.to_string();
        let Some(t) = self.tabs.get(tab) else { return Ok(()) };
        let (text, profile, console) = (t.editor.text(), t.saved_profile(), t.doc.console_id.clone());
        let was_console = t.doc.script.is_none();
        let written = if replace { store.save(&path, &text, None) } else { store.create(&path, &text) };
        let stamp = match written {
            Ok(s) => s,
            Err(scripts::SaveError::Exists(p)) => return Err(exists_label(store, &p)),
            Err(e) => {
                let error = self.reason_text(&Self::save_reason(&e));
                self.flash(Notice::new(Msg::ScriptsSaveFailed { error }, Level::Error));
                return Ok(());
            }
        };
        if let Some(store) = self.scripts.as_mut() {
            let _ = store.bind(&path, profile);
        }
        let shared = self.tabs.iter().any(|o| o.id != tab && o.doc.console_id == console);
        if was_console
            && !shared
            && let Some(state) = self.state_dir()
        {
            // Its text is in the new saved query now. Should the file stay (it cannot be
            // removed), the next launch recovers it as a tab: a copy, nothing lost.
            let _ = datarig_core::workspace::remove_console(&state, &console);
        }
        if let Some(t) = self.tabs.get_mut(tab) {
            t.kind = TabKind::Script;
            t.doc.script = Some(path.clone());
            t.doc.saved = text;
            t.doc.written = true;
            t.doc.stamp = Some(stamp);
            t.doc.save_due = None;
            t.doc.save_error = None;
            t.doc.conflict = false;
        }
        self.reveal_script(&path);
        self.save_workspace();
        self.flash(Notice::new(Msg::ScriptsSaved { name: display_name(&path, false).to_string() }, Level::Success));
        Ok(())
    }

    /// Open saved query `path` in a tab bound to the profile it last ran on: the tab it is
    /// open in already, else a new one. The editor gets the focus, and the profile connects
    /// then (lazily, like a restored tab). A profile that is not among the profiles gives no
    /// connection, and the binding is kept as it is.
    pub(super) fn open_script(&mut self, path: &str) {
        if let Some(t) = self.tabs.find_script(path) {
            self.switch_tab(|m| m.position(t).is_some_and(|i| m.activate(i)));
            self.focus = Focus::Editor;
            return;
        }
        let Some(store) = self.scripts.as_ref() else { return };
        let (text, stamp) = match store.read(path) {
            Ok(r) => r,
            Err(e) => {
                self.refresh_scripts();
                let name = path.to_string();
                let error = self.io_text(&e);
                return self.flash(Notice::new(Msg::ScriptsOpenFailed { name, error }, Level::Error));
            }
        };
        let binding = store.binding(path);
        let profile = binding.filter(|p| self.profile(*p).is_some());
        self.leave_tab();
        let id = self.tabs.open(TabKind::Script, profile, Editor::new(&text));
        if let Some(t) = self.tabs.get_mut(id) {
            t.doc.script = Some(path.to_string());
            t.doc.saved = text;
            t.doc.written = true;
            t.doc.stamp = Some(stamp);
            t.doc.lazy_connect = profile.is_some();
            t.doc.kept_profile = binding.filter(|_| profile.is_none());
        }
        self.entered_tab();
        self.focus = Focus::Editor;
    }

    /// `:e <name>`: the saved query of that name (ignoring case, without its extension).
    pub(super) fn open_script_named(&mut self, input: &str) -> Result<(), Notice> {
        let found = match scripts::name::parse_name(input, scripts::SQL).ok().zip(self.scripts.as_ref()) {
            Some((p, s)) => s.find(&p).map_err(|u| {
                let error = self.io_text(&std::io::Error::from(u.kind));
                Notice::new(Msg::ScriptsOpenFailed { name: input.to_string(), error }, Level::Error)
            })?,
            None => None,
        };
        let found = found.filter(|p| self.scripts.as_ref().is_some_and(|s| s.stamp(p).is_some()));
        match found {
            Some(p) => {
                self.open_script(&p);
                Ok(())
            }
            None => Err(Notice::new(Msg::ScriptsUnknown { name: input.to_string() }, Level::Error)),
        }
    }

    /// The saved query of tab `id` changed on disk (or went away, `missing`) since it was
    /// read: ask what to do; nothing is written meanwhile.
    pub(super) fn script_conflict(&mut self, id: TabId, missing: bool) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.doc.conflict = true;
        t.doc.save_due = None;
        let name = t.script().map(|p| display_name(p, false).to_string()).unwrap_or_default();
        if self.overlays.confirm().is_some_and(|c| matches!(c.action, ConfirmAction::ScriptConflict(_))) {
            return;
        }
        let (text, keys) = if missing {
            (Msg::ScriptsConflictMissing { name }, Label::ScriptsConflictMissingKeys)
        } else {
            (Msg::ScriptsConflictChanged { name }, Label::ScriptsConflictKeys)
        };
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ScriptsConflictTitle,
            text,
            details: Vec::new(),
            keys,
            action: ConfirmAction::ScriptConflict(id),
            folder: None,
            path: None,
        }));
    }

    /// Keys of the conflict question: `r` reload from disk, `o` write the tab's text over the
    /// file, `Esc` later.
    pub(super) fn conflict_key(&mut self, id: TabId, key: KeyEvent) {
        match key.code {
            KeyCode::Char('r') => {
                self.overlays.close(OverlayKind::Confirm);
                self.reload_script(id);
            }
            KeyCode::Char('o') => {
                self.overlays.close(OverlayKind::Confirm);
                let Some(path) = self.tabs.get(id).and_then(|t| t.script().map(str::to_string)) else { return };
                let text = self.tabs.get(id).map(|t| t.editor.text()).unwrap_or_default();
                let r = self.scripts.as_ref().map(|s| s.save(&path, &text, None));
                match r {
                    Some(Ok(stamp)) => {
                        if let Some(t) = self.tabs.get_mut(id) {
                            t.doc.conflict = false;
                            t.doc.stamp = Some(stamp);
                            t.doc.saved = text;
                            t.doc.written = true;
                        }
                        self.refresh_scripts();
                        let name = display_name(&path, false).to_string();
                        self.flash(Notice::new(Msg::ScriptsSaved { name }, Level::Success));
                    }
                    Some(Err(e)) => {
                        let error = self.reason_text(&Self::save_reason(&e));
                        self.flash(Notice::new(Msg::ScriptsSaveFailed { error }, Level::Error))
                    }
                    None => {}
                }
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                self.overlays.close(OverlayKind::Confirm);
                self.flash(Notice::new(Label::ScriptsConflictLater, Level::Warning));
            }
            _ => {}
        }
    }

    /// Replace tab `id`'s text with its file's (the edits since the last save are dropped).
    fn reload_script(&mut self, id: TabId) {
        let Some(path) = self.tabs.get(id).and_then(|t| t.script().map(str::to_string)) else { return };
        let (text, stamp) = match self.scripts.as_ref().map(|s| s.read(&path)) {
            Some(Ok(r)) => r,
            Some(Err(e)) => {
                let (name, error) = (path.clone(), self.io_text(&e));
                return self.flash(Notice::new(Msg::ScriptsOpenFailed { name, error }, Level::Error));
            }
            None => return,
        };
        if let Some(t) = self.tabs.get_mut(id) {
            let (row, col) = (t.editor.row, t.editor.col);
            t.editor = Editor::new(&text);
            t.editor.row = row.min(t.editor.lines.len() - 1);
            t.editor.col = col;
            t.doc.saved = text;
            t.doc.written = true;
            t.doc.stamp = Some(stamp);
            t.doc.conflict = false;
            t.popup = None;
        }
        let name = display_name(&path, false).to_string();
        self.flash(Notice::new(Msg::ScriptsReloaded { name }, Level::Info));
    }

    /// Tabs with a saved query whose edits are not on disk because of a conflict.
    pub(super) fn conflicted(&self, only: Option<TabId>) -> usize {
        self.tabs.iter().filter(|t| only.is_none_or(|o| o == t.id) && t.doc.conflict && t.dirty()).count()
    }

    // ── the explorer's section ──────────────────────────────────────────────

    /// Open the section and the folders around `path` and put the cursor on it.
    pub(super) fn reveal_script(&mut self, path: &str) {
        self.refresh_scripts();
        self.scripts_expanded = true;
        let mut dir = scripts::parent(path);
        while let Some(d) = dir {
            self.script_folders.insert(d.to_string());
            dir = scripts::parent(d);
        }
        self.explorer.select_kind(explorer::RowKind::Script(path.to_string()));
    }

    /// `R` on a saved query or one of its folders: the name dialog.
    pub(super) fn open_rename_script(&mut self, path: String, folder: bool) {
        let (title, text, purpose) = if folder {
            (Msg::NameRenameScriptFolder { name: path.clone() }, path.clone(), NamePurpose::RenameScriptFolder(path))
        } else {
            let name = display_name(&path, false).to_string();
            (Msg::NameRenameScript { name }, stem_path(&path).to_string(), NamePurpose::RenameScript(path))
        };
        self.overlays.push(Overlay::NameInput(NameInput { title, input: TextInput::new(&text), error: None, purpose }));
    }

    /// Rename (or move) saved query or folder `from` to what the user typed; its open tabs
    /// follow.
    pub(super) fn rename_script(&mut self, from: &str, input: &str, folder: bool) -> Result<(), Label> {
        let Some(store) = self.scripts.as_ref() else { return Err(Label::ScriptsNoDataDir) };
        let to = if folder { store.resolve_folder(input, Some(from)) } else { store.resolve_new(input, Some(from)) }
            .map_err(|e| name_error(store, &e))?;
        if to == from {
            return Ok(());
        }
        if folder && is_within(&to, from) {
            return Err(Label::ValidateScriptIntoItself);
        }
        // Open tabs write their edits first.
        let open: Vec<TabId> =
            self.tabs.iter().filter(|t| t.script().is_some_and(|p| is_within(p, from))).map(|t| t.id).collect();
        for id in &open {
            if self.tabs.get(*id).is_some_and(|t| t.dirty() && !t.doc.conflict) {
                self.save_tab(*id);
            }
        }
        let Some(store) = self.scripts.as_mut() else { return Ok(()) };
        if let Err(e) = store.rename(from, &to) {
            let (name, error) = (from.to_string(), self.io_text(&e));
            self.flash(Notice::new(Msg::ScriptsRenameFailed { name, error }, Level::Error));
            return Ok(());
        }
        for id in open {
            let Some(t) = self.tabs.get_mut(id) else { continue };
            if let Some(p) = t.doc.script.clone() {
                let moved = format!("{to}{}", &p[from.len()..]);
                t.doc.stamp = self.scripts.as_ref().and_then(|s| s.stamp(&moved));
                t.doc.script = Some(moved);
            }
        }
        if folder {
            let renamed: Vec<String> = self.script_folders.iter().filter(|f| is_within(f, from)).cloned().collect();
            for f in renamed {
                self.script_folders.remove(&f);
                self.script_folders.insert(format!("{to}{}", &f[from.len()..]));
            }
        }
        self.reveal_script(&to);
        if folder {
            self.explorer.select_kind(explorer::RowKind::ScriptFolder(to.clone()));
        }
        self.save_workspace();
        let (from_t, to_t) = (from.to_string(), to.clone());
        self.flash(Notice::new(Msg::ScriptsRenamed { from: from_t, to: to_t }, Level::Success));
        Ok(())
    }

    /// `m` on a saved query or folder: pick the folder to move it to.
    pub(super) fn open_move_script(&mut self, path: String) {
        let mut items = vec![(None, self.i18n.label(Label::ChooserFolderTop).to_string())];
        items.extend(
            self.script_list
                .iter()
                .filter(|e| e.folder && !is_within(&e.path, &path))
                .map(|e| (Some(e.path.clone()), format!("{}/", e.path))),
        );
        let current = scripts::parent(&path).map(str::to_string);
        let selected = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
        self.overlays.push(Overlay::Chooser(Chooser {
            title: Label::ChooserMoveTitle,
            items,
            selected,
            filter: TextInput::default(),
            filtering: false,
            purpose: ChooserPurpose::MoveScript(path),
        }));
    }

    /// Move saved query or folder `path` into `folder` (`None`: the top).
    pub(super) fn move_script(&mut self, path: &str, folder: Option<String>) {
        let is_folder = self.script_list.iter().any(|e| e.folder && e.path == path);
        let name = path.rsplit('/').next().unwrap_or(path);
        let name = if is_folder { name.to_string() } else { stem_path(name).to_string() };
        let target = match folder {
            Some(f) => format!("{f}/{name}"),
            None => name,
        };
        if let Err(l) = self.rename_script(path, &target, is_folder) {
            self.flash(Notice::new(l, Level::Warning));
        }
    }

    /// `Space s o`, `:e`: the folder tree of the saved queries, with a filter.
    pub(super) fn open_script_chooser(&mut self) {
        self.refresh_scripts();
        if !self.script_list.iter().any(|e| !e.folder) {
            return self.flash(Notice::new(Label::ScriptsNone, Level::Info));
        }
        self.open_script_tree(super::script_tree::TreeMode::Open);
    }

    /// A new folder `typed` of the saved queries inside `parent` (the tree dialog's `n`).
    pub(super) fn new_script_folder(&mut self, parent: Option<String>, typed: &str) -> Result<(), Label> {
        let Some(store) = self.scripts.as_ref() else { return Err(Label::ScriptsNoDataDir) };
        let input = match &parent {
            Some(p) => format!("{p}/{typed}"),
            None => typed.to_string(),
        };
        let path = store.resolve_folder(&input, None).map_err(|e| match e {
            NameError::Exists(_) => Label::ValidateFolderExists,
            e => name_error(store, &e),
        })?;
        if let Err(e) = store.create_folder(&path) {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(Label::ValidateFolderExists);
            }
            let (name, error) = (path.clone(), self.io_text(&e));
            self.flash(Notice::new(Msg::ScriptsFolderFailed { name, error }, Level::Error));
            return Ok(());
        }
        self.tree_folder_made(&path);
        Ok(())
    }

    /// Save tab `tab`, a saved query, as `Ctrl+S` does (the tree dialog named its own file).
    pub(super) fn save_current_in(&mut self, tab: TabId) {
        if self.tab().id != tab {
            self.switch_tab(|m| m.position(tab).is_some_and(|i| m.activate(i)));
        }
        self.save_current();
    }

    /// `d` on a saved query (or `Space s d` on its tab): ask first.
    pub(super) fn request_delete_script(&mut self, path: String) {
        let open = self.tabs.find_script(&path).is_some();
        let details = if open { vec![Msg::Label(Label::ScriptsDeleteOpenTab)] } else { Vec::new() };
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::ScriptsDeleteConfirm { name: stem_path(&path).to_string() },
            details,
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteScript,
            folder: None,
            path: Some(path),
        }));
    }

    /// `d` on a folder of the saved queries: an empty one is deleted after a confirmation.
    pub(super) fn request_delete_script_folder(&mut self, path: String) {
        match self.scripts.as_ref().map(|s| s.folder_empty(&path)) {
            Some(Ok(true)) => {}
            Some(Err(e)) => {
                let error = self.io_text(&e);
                return self.flash(Notice::new(Msg::ScriptsDeleteFailed { name: path, error }, Level::Error));
            }
            _ => return self.flash(Notice::new(Msg::ExplorerFolderNotEmpty { name: path }, Level::Warning)),
        }
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::ExplorerDeleteFolder { name: path.clone() },
            details: Vec::new(),
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteScriptFolder,
            folder: None,
            path: Some(path),
        }));
    }

    /// Delete saved query `path` (confirmed). Its open tab closes without writing the file
    /// again; `Space t u` brings its text back as a console.
    pub(super) fn delete_script(&mut self, path: &str) {
        if let Some(id) = self.tabs.find_script(path) {
            if let Some(t) = self.tabs.get_mut(id) {
                t.doc.script = None;
                t.kind = TabKind::Console;
                t.doc.saved = t.editor.text();
                t.doc.written = true;
            }
            self.close_tab(id);
        }
        let rows = self.explorer_rows();
        let at = self.explorer.index(&rows);
        let r = self.scripts.as_mut().map(|s| s.delete(path));
        self.refresh_scripts();
        let rows = self.explorer_rows();
        self.explorer.select(&rows, at.min(rows.len().saturating_sub(1)));
        self.save_workspace();
        let name = stem_path(path).to_string();
        match r {
            Some(Err(e)) => {
                let error = self.io_text(&e);
                self.flash(Notice::new(Msg::ScriptsDeleteFailed { name, error }, Level::Error))
            }
            _ => self.flash(Notice::new(Msg::ScriptsDeleted { name }, Level::Success)),
        }
    }

    pub(super) fn delete_script_folder(&mut self, path: &str) {
        let r = self.scripts.as_mut().map(|s| s.delete_folder(path));
        self.script_folders.remove(path);
        self.refresh_scripts();
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        self.explorer.select(&rows, i);
        let name = path.to_string();
        match r {
            Some(Err(e)) => {
                let error = self.io_text(&e);
                self.flash(Notice::new(Msg::ScriptsDeleteFailed { name, error }, Level::Error))
            }
            _ => self.flash(Notice::new(Msg::ExplorerFolderDeleted { name }, Level::Success)),
        }
    }

    /// The saved query of the active tab (`Space s r`, `Space s d`).
    pub(super) fn current_script(&self) -> Option<String> {
        self.tab().script().map(str::to_string)
    }
}
