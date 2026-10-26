//! Where an export is written: the tree dialog ([`ScriptTree`] in [`TreeMode::ExportPath`])
//! over a folder of the file system, opened from the export dialog's path field (`Ctrl+O`, its
//! `[Browse…]` button). It opens at the folder of the field's path, else `~/Downloads`, else
//! the home folder (`~` is `HOME`). Folders are listed as they open; `../` (or `←` on the top
//! row) goes up. Files are dimmed: they are what a file of that name would replace. Names that
//! start with `.` are left out until `.` shows them.
//!
//! Tree: `↑`/`↓` (`j`/`k`) move, `→`/`←` (`l`/`h`) open and close a folder, `Enter` on the top
//! row ("Save here") or `Ctrl+S` anywhere puts the selected folder in the field with the file
//! name the field had; `Enter` on a file puts that file in the field (Export still asks before
//! it replaces it); `n` makes a new folder inside the selected one (never one that exists),
//! `/` or `Tab` puts the keyboard on the filter, `Esc`/`q` cancel and leave the field as it was.
//! Filter: as in the folder picker, plus a text with `/` or a leading `~` is a path that
//! `Enter` opens (a folder) or takes (anything else).
//!
//! A folder that cannot be listed says so and is never shown empty; nothing is saved into it.
//! A link to a folder is opened like a folder, one level at a time; one that leads back to a
//! folder on the way to it is not opened. Nothing is read but folder listings, each on a
//! worker (see `folder_read`): a folder being read says so, one that takes too long too.
//!
//! The path field completes names too (`Tab`): the longest text the names that start with what
//! was typed share, then (`Tab` again) a list of them below the field, then each in turn.

use super::chooser::{NameInput, NamePurpose};
use super::export::expand_path;
use super::export_form::ExportField;
use super::folder_read::{self, Got, Kind, NoAnswer};
use super::key_picker::{self, Disk, What, is_path};
use super::script_tree::{ScriptTree, TreeFocus, TreeMode, TreeRow};
use super::*;
use crate::widgets::text_input::InputResult;
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::scripts::{self, display_name, is_within};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// The picker's own state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportPick {
    /// Names that start with `.` are listed.
    pub hidden: bool,
}

/// `Tab` in the path field: the names it matched, and the one put in last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathCompletion {
    /// The field's text when these were listed or last put in; another text starts over.
    pub text: String,
    /// The field's text completed to each name (a folder with `/`).
    pub matches: Vec<String>,
    pub at: Option<usize>,
    /// The folder read for these, while it is being read.
    pub wait: Option<u64>,
    /// What to say below the field instead of the names (being read, could not be read).
    pub note: Option<Notice>,
}

impl PathCompletion {
    /// The names, as the list below the field shows them.
    pub fn names(&self) -> String {
        let name = |m: &String| {
            let t = m.trim_end_matches(['/', '\\']);
            let cut = t.rfind(['/', '\\']).map_or(0, |i| i + 1);
            m[cut..].to_string()
        };
        self.matches.iter().map(name).collect::<Vec<_>>().join(" · ")
    }
}

/// The folder `text` completes in (`~/Downloads` for a bare name), the text before its last
/// part, and that part.
fn completion_dir(text: &str, home: Option<&Path>, folder: &Path) -> Option<(PathBuf, String, String)> {
    let cut = text.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let (dir_text, prefix) = text.split_at(cut);
    let dir = match dir_text {
        "" => Some(folder.to_path_buf()),
        d => expand_path(d, home, folder),
    }?;
    Some((dir, dir_text.to_string(), prefix.to_string()))
}

/// What `prefix` completes to among `entries` (of the folder `dir_text` names): the names that
/// start with it, a folder with `/`. Hidden ones only when it starts with `.`.
pub fn path_matches(entries: Vec<(String, Kind)>, dir_text: &str, prefix: &str) -> Vec<String> {
    entries
        .into_iter()
        .filter(|(n, _)| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .map(|(n, kind)| format!("{dir_text}{n}{}", if kind == Kind::Folder { "/" } else { "" }))
        .collect()
}

/// The longest text every one of `all` starts with.
fn common_prefix(all: &[String]) -> String {
    let Some(first) = all.first() else { return String::new() };
    let mut end = first.len();
    for s in &all[1..] {
        end = first
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .map(|((i, a), _)| i + a.len_utf8())
            .last()
            .unwrap_or(0)
            .min(end);
    }
    first[..end].to_string()
}

impl ScriptTree {
    /// Export path mode: `path` is listed (a hidden name only while hidden ones are shown).
    pub(super) fn export_listed(&self, path: &str) -> bool {
        matches!(self.mode, TreeMode::ExportPath(p) if p.hidden) || !path.split('/').any(|s| s.starts_with('.'))
    }

    /// With filter `q` (case folded): `path` is shown when its name or that of something listed
    /// inside it holds `q`.
    /// The paths shown with filter `q` (case folded), in one pass: each listed entry whose name
    /// holds `q`, and the folders on the way to it.
    pub(super) fn export_hits(&self, q: &str) -> HashSet<&str> {
        let mut hits = HashSet::new();
        for e in &self.entries {
            if e.path.is_empty()
                || !self.export_listed(&e.path)
                || !scripts::name::fold(display_name(&e.path, true)).contains(q)
            {
                continue;
            }
            let mut p = Some(e.path.as_str());
            while let Some(x) = p {
                if !hits.insert(x) {
                    break;
                }
                p = scripts::parent(x);
            }
        }
        hits
    }

    /// Shown at a hidden name (a hidden folder or file of the field, or the hidden folder gone
    /// up from): hidden names are listed, so the selection is a row on screen.
    pub(super) fn export_reveal(&mut self, select: Option<&str>) {
        if let TreeMode::ExportPath(p) = &mut self.mode
            && select.is_some_and(|n| n.starts_with('.') && self.entries.iter().any(|e| e.path == n))
        {
            p.hidden = true;
        }
    }

    /// The folder the selection stands for, on disk.
    pub(super) fn export_dir(&self) -> PathBuf {
        match self.folder() {
            Some(rel) => self.root.join(rel),
            None => self.root.clone(),
        }
    }

    /// List folder `rel` again (`""`: the folder shown), `select` selected.
    fn export_reload(&mut self, rel: &str, select: String) {
        if rel.is_empty() {
            return self.show_dir(self.root.clone(), Some(&select));
        }
        self.entries.retain(|e| e.path == rel || !is_within(&e.path, rel));
        self.loaded.retain(|l| l != rel && !is_within(l, rel));
        // Listed again below: it and what was open inside it are closed until then, never
        // open and empty.
        self.open.retain(|o| o != rel && !is_within(o, rel));
        // What was being read inside it (or of it) has no row to go to any more.
        self.disk.waiting.retain(|(_, w)| !w.rel().is_some_and(|r| r == rel || is_within(r, rel)));
        self.disk.slow.retain(|p| p != rel && !is_within(p, rel));
        self.disk.unknown.retain(|p| !is_within(p, rel));
        self.load(rel, Some(select));
    }

    /// The selection goes to the first row that matches the filter, unless it is on one.
    fn export_follow_filter(&mut self) {
        if !self.filtering() {
            return;
        }
        let q = scripts::name::fold(self.input.text().trim());
        let hit = |r: &TreeRow| match r {
            TreeRow::Folder(p) | TreeRow::File(p) => scripts::name::fold(display_name(p, true)).contains(&q),
            TreeRow::Up | TreeRow::Top => false,
        };
        if hit(&self.row) {
            return;
        }
        if let Some(i) = self.rows().iter().position(|(r, _)| hit(r)) {
            self.select(i);
        }
    }
}

impl App {
    /// `Ctrl+O` or `[Browse…]` on the export dialog: open the picker at the field's folder.
    pub(super) fn open_export_path_pick(&mut self) {
        let (home, folder) = (self.home(), self.export_folder());
        let Some(f) = self.overlays.export_form_mut() else { return };
        f.focus = ExportField::Path;
        f.completion = None;
        let typed = f.path.text().trim().to_string();
        let current = expand_path(&typed, home.as_deref(), &folder);
        // The field's folder, else the folder of its file, else ~/Downloads, else home (asked
        // on the worker, in this order).
        let (mut dirs, select) = key_picker::start_dirs(current.as_deref());
        for d in [Some(folder), home].into_iter().flatten() {
            dirs.push((d, select.clone()));
        }
        dirs.push((PathBuf::from(std::path::MAIN_SEPARATOR_STR), select));
        let disk = Disk::new(self.folder_reader());
        let mut t = ScriptTree {
            mode: TreeMode::ExportPath(ExportPick::default()),
            entries: Vec::new(),
            open: BTreeSet::new(),
            row: TreeRow::Top,
            focus: TreeFocus::Tree,
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

    /// Keys of the picker (`overlay.script_tree`, `overlay.script_tree.name`).
    pub(super) fn export_path_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if t.focus == TreeFocus::Name {
            match key.code {
                KeyCode::Esc => {
                    t.input.set("");
                    t.focus = TreeFocus::Tree;
                }
                KeyCode::Enter if !repeat && is_path(t.input.text().trim()) => self.export_path_typed(),
                KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => t.focus = TreeFocus::Tree,
                KeyCode::Up => t.step(-1),
                KeyCode::Down => t.step(1),
                KeyCode::Char('s') if ctrl && !repeat => self.export_path_save_here(),
                _ => {
                    if t.input.handle_key(&key) == InputResult::Changed {
                        t.error = None;
                        t.export_follow_filter();
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => t.step(-1),
            KeyCode::Down | KeyCode::Char('j') => t.step(1),
            KeyCode::Left | KeyCode::Char('h') if matches!(t.row, TreeRow::Up | TreeRow::Top) => t.up(),
            KeyCode::Right | KeyCode::Char('l') if t.row == TreeRow::Up => t.up(),
            KeyCode::Right | KeyCode::Char('l') => t.set_open(Some(true)),
            KeyCode::Left | KeyCode::Char('h') => {
                if t.is_open(&t.row) == Some(true) && !t.filtering() {
                    t.set_open(Some(false));
                } else {
                    let rel = match &t.row {
                        TreeRow::Folder(p) | TreeRow::File(p) => scripts::parent(p).map(str::to_string),
                        TreeRow::Up | TreeRow::Top => None,
                    };
                    t.row = rel.map_or(TreeRow::Top, TreeRow::Folder);
                }
            }
            _ if repeat => {}
            KeyCode::Char('s') if ctrl => self.export_path_save_here(),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('/') => t.focus = TreeFocus::Name,
            KeyCode::Esc | KeyCode::Char('q') => self.overlays.close(OverlayKind::ScriptTree),
            KeyCode::Enter => self.export_path_enter(),
            KeyCode::Char('.') => {
                if let TreeMode::ExportPath(p) = &mut t.mode {
                    p.hidden = !p.hidden;
                }
                // A hidden row that was selected goes with it.
                if !t.rows().iter().any(|(r, _)| *r == t.row) {
                    t.row = TreeRow::Top;
                }
            }
            KeyCode::Char('n') => self.export_path_new_folder(),
            _ => {}
        }
    }

    /// `Enter` (or a second click): `../` goes up, the top row saves here, a folder opens or
    /// closes, a file is picked.
    pub(super) fn export_path_enter(&mut self) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        match t.row.clone() {
            TreeRow::Up => t.up(),
            TreeRow::Top => self.export_path_save_here(),
            TreeRow::Folder(_) => t.set_open(None),
            TreeRow::File(p) => {
                let path = t.root.join(p);
                self.export_path_picked(path);
            }
        }
    }

    /// A path typed in the filter: a folder is shown, anything else is taken as the file
    /// (which it is is asked on a worker).
    fn export_path_typed(&mut self) {
        let (home, folder) = (self.home(), self.export_folder());
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let text = t.input.text().trim().to_string();
        let Some(path) = expand_path(&text, home.as_deref(), &folder) else { return };
        // The latest `Enter` is the one answered.
        t.disk.waiting.retain(|(_, w)| !matches!(w, What::Typed { .. }));
        let p = path.clone();
        t.error = None;
        t.ask(What::Typed { path: path.clone(), text }, path, move |fs| Got::IsDir(fs.is_dir(&p)));
    }

    /// The path typed in the filter is a folder (`dir`) or not.
    pub(super) fn export_path_typed_is(&mut self, path: PathBuf, dir: bool) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        if dir {
            t.input.set("");
            t.focus = TreeFocus::Tree;
            return t.show_dir(path, None);
        }
        self.export_path_picked(path);
    }

    /// "Save here" (`Ctrl+S`): the selected folder, with the file name the field has (the one
    /// made for the format when it names no file). Whether the folder can be listed, and
    /// whether the field names a folder, is asked on a worker.
    fn export_path_save_here(&mut self) {
        let (home, folder) = (self.home(), self.export_folder());
        let Some(f) = self.overlays.export_form() else { return };
        let typed = f.path.text().trim().to_string();
        let typed_path = if typed.is_empty() || typed.ends_with(['/', '\\']) {
            None
        } else {
            expand_path(&typed, home.as_deref(), &folder)
        };
        let Some(t) = self.overlays.script_tree_mut() else { return };
        let (rel, dir) = (t.folder().unwrap_or_default(), t.export_dir());
        // While the folder shown is first read, which one it is is not known yet.
        if t.disk.reading_shown() {
            return;
        }
        // The latest one is the one answered.
        t.disk.waiting.retain(|(_, w)| !matches!(w, What::SaveHere { .. }));
        let d = dir.clone();
        t.error = None;
        t.ask(What::SaveHere { rel, dir: dir.clone() }, dir, move |fs| Got::Readable {
            // Unknown is not empty: a folder that cannot be listed is not saved into.
            result: fs.readable(&d).map_err(|e| datarig_core::fault::Fault::io_at(&e, &d)),
            typed_dir: typed_path.is_some_and(|p| fs.is_dir(&p)),
        });
    }

    /// Save into folder `dir` (it can be listed); `typed_dir`: the field names a folder.
    pub(super) fn export_path_save_into(&mut self, dir: PathBuf, typed_dir: bool) {
        let Some(f) = self.overlays.export_form() else { return };
        let typed = f.path.text().trim();
        // A field that names a folder (with or without `/`) has no file name of its own.
        let folder_typed = typed.is_empty() || typed.ends_with(['/', '\\']) || typed_dir;
        let named = if folder_typed { f.default_text(f.format) } else { typed.to_string() };
        let cut = named.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let name = named[cut..].to_string();
        self.export_path_picked(dir.join(name));
    }

    /// Put `path` in the export dialog's field (`~/…` under the home folder) and close the
    /// picker.
    fn export_path_picked(&mut self, path: PathBuf) {
        let text = key_picker::shown(&path, self.home().as_deref());
        self.overlays.close(OverlayKind::ScriptTree);
        if let Some(f) = self.overlays.export_form_mut() {
            f.set_path(&text);
            f.focus = ExportField::Path;
        }
    }

    /// `n`: name a new folder inside the selected one (not in one that cannot be read: asked on
    /// a worker).
    fn export_path_new_folder(&mut self) {
        let Some(t) = self.overlays.script_tree_mut() else { return };
        if t.disk.reading_shown() {
            return;
        }
        t.disk.waiting.retain(|(_, w)| !matches!(w, What::NewFolder { .. }));
        let (parent, dir) = (t.folder(), t.export_dir());
        let d = dir.clone();
        t.error = None;
        // Not in a folder that cannot be listed (the new one could not be shown).
        t.ask(What::NewFolder { parent, dir: dir.clone() }, dir, move |fs| Got::Readable {
            result: fs.readable(&d).map_err(|e| datarig_core::fault::Fault::io_at(&e, &d)),
            typed_dir: false,
        });
    }

    /// Ask for the name of a new folder inside `parent` (it can be listed).
    pub(super) fn export_path_name_folder(&mut self, parent: Option<String>) {
        let title = match &parent {
            Some(p) => Msg::NameNewFolderIn { parent: p.clone() },
            None => Msg::Label(Label::NameNewFolder),
        };
        self.overlays.push(Overlay::NameInput(NameInput {
            title,
            input: TextInput::default(),
            error: None,
            purpose: NamePurpose::NewExportFolder { parent },
            buttons: Default::default(),
        }));
    }

    /// Make folder `typed` inside `parent` (relative to the folder shown), selected. A name
    /// that is taken is refused, never reused.
    pub(super) fn new_export_folder(&mut self, parent: Option<String>, typed: &str) -> Result<(), Label> {
        match typed {
            "" => return Err(Label::ValidateFolderEmpty),
            "." | ".." => return Err(Label::ValidateFolderDots),
            t if t.contains(['/', '\\']) => return Err(Label::ValidateExportFolderOne),
            _ => {}
        }
        let Some(t) = self.overlays.script_tree_mut() else { return Ok(()) };
        let dir = match &parent {
            Some(p) => t.root.join(p).join(typed),
            None => t.root.join(typed),
        };
        // Made on a worker; the prompt stays until it answers (a name taken is said there).
        if t.disk.waiting.iter().any(|(_, w)| matches!(w, What::Create { .. })) {
            return Ok(());
        }
        let d = dir.clone();
        let what = What::Create { parent, name: typed.to_string(), dir: dir.clone() };
        t.ask(what, dir, move |fs| Got::Made(fs.create_dir(&d).map_err(|e| Fault::io_at(&e, &d))));
        Ok(())
    }

    /// Folder `name` inside `parent` was made (`made`), or not: listed again and selected; a
    /// name that is taken is refused in the prompt, never reused.
    pub(super) fn export_folder_made(&mut self, parent: Option<String>, name: String, made: Result<(), Fault>) {
        if let Err(f) = made {
            if f.kind == FaultKind::Io(std::io::ErrorKind::AlreadyExists) {
                if let Some(n) = self.overlays.name_input_mut() {
                    n.error = Some(Label::ValidateFolderExists);
                }
                return;
            }
            self.overlays.close(OverlayKind::NameInput);
            let error = self.fault_text("io", &f);
            self.flash(Notice::new(Msg::ScriptsFolderFailed { name, error }, Level::Error));
            return;
        }
        self.overlays.close(OverlayKind::NameInput);
        let Some(t) = self.overlays.script_tree_mut() else { return };
        if name.starts_with('.')
            && let TreeMode::ExportPath(p) = &mut t.mode
        {
            p.hidden = true;
        }
        let rel = parent.clone().unwrap_or_default();
        let path = match &parent {
            Some(p) => format!("{p}/{name}"),
            None => name.clone(),
        };
        t.input.set("");
        t.error = None;
        t.export_reload(&rel, if rel.is_empty() { name } else { path });
        t.focus = TreeFocus::Tree;
    }

    /// `Tab` in the export dialog's path field: complete the name typed, then list the names it
    /// may be, then put each in turn. `false`: nothing to complete (the focus moves on).
    pub(super) fn export_path_complete(&mut self) -> bool {
        let (home, folder) = (self.home(), self.export_folder());
        let Some(f) = self.overlays.export_form_mut() else { return false };
        let text = f.path.text().to_string();
        if text.trim().is_empty() {
            f.completion = None;
            return false;
        }
        if f.completion.as_ref().is_some_and(|c| c.text == text && c.wait.is_some()) {
            return true;
        }
        // Its folder could not be read, as said below the field: `Tab` again moves on.
        if f.completion.as_ref().is_some_and(|c| c.text == text && c.note.is_some()) {
            f.completion = None;
            return false;
        }
        if let Some(c) = f.completion.as_mut().filter(|c| c.text == text && c.matches.len() > 1) {
            let i = c.at.map_or(0, |i| (i + 1) % c.matches.len());
            c.at = Some(i);
            c.text = c.matches[i].clone();
            let next = c.text.clone();
            f.set_path(&next);
            return true;
        }
        if text == "~" {
            if home.is_none() {
                f.completion = None;
                return false;
            }
            f.set_path("~/");
            f.completion = None;
            return true;
        }
        let Some((dir, ..)) = completion_dir(&text, home.as_deref(), &folder) else {
            f.completion = None;
            return false;
        };
        // The folder is read on a worker; until it answers, the field says so.
        let path = key_picker::shown_dir(&dir, home.as_deref());
        let reader = self.folder_reader();
        let d = dir.clone();
        let id = reader.ask(dir, move |fs| Got::Listed(folder_read::sorted(fs, &d)));
        let Some(f) = self.overlays.export_form_mut() else { return true };
        let note = Notice::new(Msg::FolderReading { path }, Level::Info);
        f.completion = Some(PathCompletion { text, matches: Vec::new(), at: None, wait: Some(id), note: Some(note) });
        true
    }

    /// The folder `Tab` read answered: complete the name typed, or list the names it may be,
    /// or (none, or no such folder) move the focus on; a folder that could not be read says so
    /// below the field, and the focus stays (`Tab` again moves it on).
    /// The field's text changed meanwhile: the answer is dropped.
    pub(super) fn export_path_completed(&mut self, got: Result<Got, NoAnswer>) {
        let (home, folder) = (self.home(), self.export_folder());
        let Some(f) = self.overlays.export_form() else { return };
        let Some(c) = f.completion.clone() else { return };
        let text = c.text.clone();
        let current = f.path.text() == text;
        let Some((dir, dir_text, prefix)) = completion_dir(&text, home.as_deref(), &folder) else { return };
        let path = key_picker::shown_dir(&dir, home.as_deref());
        let note = match got {
            Ok(Got::Listed(Ok(items))) => Some(Ok(path_matches(items, &dir_text, &prefix))),
            // No such folder: nothing to complete (that is known, not unknown).
            Ok(Got::Listed(Err(fault)))
                if matches!(
                    fault.kind,
                    FaultKind::Io(std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
                ) =>
            {
                Some(Ok(Vec::new()))
            }
            Ok(Got::Listed(Err(fault))) => {
                let error = self.reason_text(&crate::app::fault_reason(&fault));
                Some(Err(Notice::new(Msg::ExportPathCompleteUnreadable { path, error }, Level::Warning)))
            }
            Err(NoAnswer::TooSlow) => Some(Err(Notice::new(Msg::ExportPathCompleteTooSlow { path }, Level::Warning))),
            Err(NoAnswer::Broke) => {
                let error = self.reason_text(&Label::FaultOther.into());
                Some(Err(Notice::new(Msg::ExportPathCompleteUnreadable { path, error }, Level::Warning)))
            }
            Ok(_) => None,
        };
        let Some(f) = self.overlays.export_form_mut() else { return };
        f.completion = None;
        // The text changed, or the focus left the field: nothing is put in.
        if !current || f.focus != ExportField::Path {
            return;
        }
        match note {
            Some(Ok(matches)) => {
                let common = common_prefix(&matches);
                if common.len() > text.len() && common.starts_with(&text) {
                    f.set_path(&common);
                } else if matches.len() > 1 {
                    f.completion = Some(PathCompletion { text, matches, ..Default::default() });
                } else {
                    // Nothing to complete: `Tab` moves on.
                    f.step_focus(1);
                }
            }
            Some(Err(n)) => f.completion = Some(PathCompletion { text, note: Some(n), ..Default::default() }),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datarig_core::scripts::Entry;

    #[test]
    fn the_common_prefix_stops_where_two_names_part() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(common_prefix(&v(&["a/report-1.csv", "a/report-2.csv"])), "a/report-");
        assert_eq!(common_prefix(&v(&["one"])), "one");
        assert_eq!(common_prefix(&v(&["\u{e9}a", "\u{e9}b"])), "\u{e9}");
        assert_eq!(common_prefix(&[]), "");
    }

    /// Filtering is one pass over the entries, not one per entry: 40,000 names filter well
    /// inside the budget (a pass per entry takes over a billion steps here).
    #[test]
    fn filtering_a_large_folder_is_linear() {
        let mut entries = Vec::new();
        for i in 0..200 {
            entries.push(Entry { path: format!("d{i}"), folder: true, unreadable: false });
            for j in 0..199 {
                entries.push(Entry { path: format!("d{i}/f{j}.csv"), folder: false, unreadable: false });
            }
        }
        let t = ScriptTree {
            mode: TreeMode::ExportPath(ExportPick::default()),
            entries,
            open: BTreeSet::new(),
            row: TreeRow::Top,
            focus: TreeFocus::Name,
            input: TextInput::new("f19"),
            error: None,
            scroll: 0,
            detached: false,
            list: Rect::default(),
            overwrite: None,
            root: PathBuf::from("/"),
            loaded: BTreeSet::new(),
            moving: None,
            disk: Default::default(),
        };
        let start = std::time::Instant::now();
        let rows = t.rows();
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        // The top row (`/` has no `../`), then each folder with `f19` and `f190`..`f198`.
        assert_eq!(rows.len(), 1 + 200 * 11);
    }
}
