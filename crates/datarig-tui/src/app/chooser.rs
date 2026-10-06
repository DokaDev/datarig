//! Small pickers of the explorer and the profile form: the chooser (a filtered list for a
//! profile's color, icon or folder, and for `m` moving a profile to a folder), the name input
//! (`N` new folder, `R` rename a folder), and a pasted connection URL.

use super::*;
use crate::app::profiles::Field;
use crate::widgets::text_input::InputResult;
use datarig_core::profile::folder::{FolderError, FolderPath};

/// What picking an item of the chooser does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChooserPurpose {
    /// Set this picker field of the profile form.
    Form(Field),
    /// Move this profile to the picked folder (`m`).
    MoveProfile(ProfileId),
    /// Move this saved query or folder to the picked folder (`m`).
    MoveScript(String),
    /// Bring back the picked closed console from the trash (`:recover`).
    Recover,
}

pub struct Chooser {
    pub title: Label,
    /// `(value, text)`: `None` is the automatic color, the driver's icon or the top level.
    pub items: Vec<(Option<String>, String)>,
    /// Index into `visible()`.
    pub selected: usize,
    pub filter: TextInput,
    /// The `/` filter has the keyboard (`overlay.chooser.filter`).
    pub filtering: bool,
    pub purpose: ChooserPurpose,
    /// First row shown, and where the rows were drawn (mouse), kept by the renderer.
    pub scroll: usize,
    pub list: ratatui::layout::Rect,
    /// The row under the pointer (highlighted; the selection does not move to it).
    pub hover: Option<usize>,
    /// The row a press armed (it is picked on the release).
    pub press: super::overlay::Press<usize>,
}

impl Chooser {
    /// Indices of the items the filter lets through (a part of the text, ignoring case).
    pub fn visible(&self) -> Vec<usize> {
        let q = self.filter.text().trim().to_lowercase();
        (0..self.items.len()).filter(|&i| q.is_empty() || self.items[i].1.to_lowercase().contains(&q)).collect()
    }

    pub(super) fn step(&mut self, d: isize) {
        let n = self.visible().len();
        if n > 0 {
            self.selected = (self.selected as isize + d).clamp(0, n as isize - 1) as usize;
        }
    }
}

/// What the name input names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamePurpose {
    /// A new folder inside `parent` (`None`: at the top).
    NewFolder { parent: Option<FolderPath> },
    /// Rename this folder (its profiles and subfolders follow).
    RenameFolder(FolderPath),
    /// A new folder of the saved queries inside `parent` (`None`: at the top), from the tree
    /// dialog of "save as".
    NewScriptFolder { parent: Option<String> },
    /// Rename (or move, with `/`) this saved query.
    RenameScript(String),
    /// Rename this folder of the saved queries.
    RenameScriptFolder(String),
    /// The name of the tunnel preset the profile form's own tunnel becomes when it is saved.
    SaveAsTunnel,
}

pub struct NameInput {
    pub title: Msg,
    pub input: TextInput,
    /// Why the last `Enter` did not apply; typing clears it.
    pub error: Option<Label>,
    pub purpose: NamePurpose,
    pub buttons: super::overlay::Buttons,
}

fn folder_error(e: &FolderError) -> Label {
    match e {
        FolderError::Empty | FolderError::EmptySegment => Label::ValidateFolderEmpty,
        FolderError::Spaces => Label::ValidateFolderSpaces,
        FolderError::Dots => Label::ValidateFolderDots,
    }
}

impl App {
    /// Open the chooser of profile form field `f` (color, icon, folder).
    pub(super) fn open_form_chooser(&mut self, f: Field) {
        let Some(form) = self.overlays.form() else { return };
        let (title, none) = match f {
            Field::Color => (Label::FormFieldColor, Label::ChooserColorAuto),
            Field::Icon => (Label::FormFieldIcon, Label::ChooserIconDriver),
            _ => (Label::FormFieldFolder, Label::ChooserFolderTop),
        };
        let current = form.choice(f);
        let items: Vec<(Option<String>, String)> = form
            .choices(f)
            .into_iter()
            .map(|v| {
                let text = match (&v, f) {
                    (None, _) => self.i18n.label(none).to_string(),
                    (Some(v), Field::Folder) => format!("{v}/"),
                    (Some(v), _) => v.clone(),
                };
                (v, text)
            })
            .collect();
        let selected = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
        self.overlays.push(Overlay::Chooser(Chooser {
            title,
            items,
            selected,
            filter: TextInput::default(),
            filtering: false,
            scroll: 0,
            list: Default::default(),
            hover: None,
            press: Default::default(),
            purpose: ChooserPurpose::Form(f),
        }));
    }

    /// `m`: move the selected profile to a folder picked from the list.
    pub(super) fn open_move(&mut self) {
        if let Some(explorer::RowKind::Script(p) | explorer::RowKind::ScriptFolder(p)) =
            self.explorer_row().map(|r| r.kind)
        {
            return self.open_move_script(p);
        }
        let Some(id) = self.selected_profile() else { return };
        let current = self.profile(id).and_then(|p| p.folder.clone());
        let mut items = vec![(None, self.i18n.label(Label::ChooserFolderTop).to_string())];
        items.extend(self.folders.iter().map(|f| (Some(f.to_string()), format!("{f}/"))));
        let selected = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
        self.overlays.push(Overlay::Chooser(Chooser {
            title: Label::ChooserMoveTitle,
            items,
            selected,
            filter: TextInput::default(),
            filtering: false,
            scroll: 0,
            list: Default::default(),
            hover: None,
            press: Default::default(),
            purpose: ChooserPurpose::MoveProfile(id),
        }));
    }

    /// Keys of the chooser list (`overlay.chooser`) and its filter (`overlay.chooser.filter`).
    pub(super) fn chooser_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(c) = self.overlays.chooser_mut() else { return };
        // The rows or the scroll may change: the highlight under the pointer goes.
        c.hover = None;
        if c.filtering {
            match key.code {
                KeyCode::Esc => {
                    c.filtering = false;
                    c.filter.set("");
                }
                KeyCode::Enter => c.filtering = false,
                KeyCode::Down => c.step(1),
                KeyCode::Up => c.step(-1),
                _ => {
                    if c.filter.handle_key(&key) == InputResult::Changed {
                        c.selected = 0;
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => c.step(1),
            KeyCode::Char('k') | KeyCode::Up => c.step(-1),
            _ if repeat => {}
            KeyCode::Char('/') => c.filtering = true,
            KeyCode::Esc | KeyCode::Char('q') => self.overlays.close(OverlayKind::Chooser),
            KeyCode::Enter => self.chooser_pick(),
            _ => {}
        }
    }

    /// `Enter` in the chooser: apply the selected item.
    pub(super) fn chooser_pick(&mut self) {
        let Some(c) = self.overlays.chooser() else { return };
        let Some(&i) = c.visible().get(c.selected) else { return };
        let (value, purpose) = (c.items[i].0.clone(), c.purpose.clone());
        self.overlays.close(OverlayKind::Chooser);
        match purpose {
            ChooserPurpose::Form(f) => {
                if let Some(form) = self.overlays.form_mut() {
                    form.set_choice(f, value);
                }
            }
            ChooserPurpose::MoveProfile(id) => self.move_profile(id, value),
            ChooserPurpose::MoveScript(path) => self.move_script(&path, value),
            ChooserPurpose::Recover => {
                if let Some(name) = value {
                    self.untrash(&name);
                }
            }
        }
    }

    /// `:recover`: the closed consoles in the trash, newest first, each with when it was
    /// closed and its first line; the picked one comes back as a tab.
    pub(super) fn open_recover(&mut self) {
        let list = match self.state_dir().map(|s| datarig_core::workspace::list_trash(&s)) {
            Some(Ok(list)) => list,
            Some(Err(e)) => {
                let error = self.io_text(&e);
                return self.flash(Notice::new(Msg::TabTrashUnreadable { error }, Level::Error));
            }
            None => Vec::new(),
        };
        if list.is_empty() {
            return self.flash(Notice::new(Label::RecoverEmpty, Level::Info));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let trash =
            self.state_dir().map(|s| s.join(datarig_core::workspace::CONSOLES).join(datarig_core::workspace::TRASH));
        let items = list
            .into_iter()
            .map(|t| {
                let age = self.age_text(now.saturating_sub(t.millis));
                let first = match trash.as_ref().map(|d| std::fs::read_to_string(d.join(&t.name))) {
                    Some(Ok(text)) => text
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .unwrap_or("")
                        .chars()
                        .take(60)
                        .collect::<String>(),
                    Some(Err(e)) => {
                        let error = self.io_text(&e);
                        self.i18n.msg(&Msg::RecoverUnreadable { error }).to_string()
                    }
                    None => String::new(),
                };
                (Some(t.name), format!("{age} · {first}"))
            })
            .collect();
        self.overlays.push(Overlay::Chooser(Chooser {
            title: Label::RecoverTitle,
            items,
            selected: 0,
            filter: TextInput::default(),
            filtering: false,
            scroll: 0,
            list: Default::default(),
            hover: None,
            press: Default::default(),
            purpose: ChooserPurpose::Recover,
        }));
    }

    /// How long ago, in words (`millis` milliseconds).
    fn age_text(&self, millis: u128) -> String {
        let minutes = (millis / 60_000) as u64;
        let msg = match minutes {
            0 => Msg::Label(Label::RecoverAgeNow),
            m if m < 60 => Msg::RecoverAgeMinutes { count: m },
            m if m < 24 * 60 => Msg::RecoverAgeHours { count: m / 60 },
            m => Msg::RecoverAgeDays { count: m / (24 * 60) },
        };
        self.i18n.msg(&msg).to_string()
    }

    /// Move profile `id` to `folder` (`None`: the top level) and save.
    fn move_profile(&mut self, id: ProfileId, folder: Option<String>) {
        let Some(p) = self.profiles.iter_mut().find(|p| p.id == id) else { return };
        p.folder = folder.clone();
        let name = p.name.clone();
        let saved = self.persist();
        self.reveal_profile(id);
        let to = folder.map_or_else(|| self.i18n.label(Label::ChooserFolderTop).to_string(), |f| format!("{f}/"));
        self.flash(saved.unwrap_or(Notice::new(Msg::ExplorerMoved { name, to }, Level::Success)));
    }

    /// `N`: a new folder in the selected folder (or the selected profile's).
    pub(super) fn open_new_folder(&mut self) {
        let parent = self.selected_folder();
        let title = match &parent {
            Some(p) => Msg::NameNewFolderIn { parent: p.to_string() },
            None => Label::NameNewFolder.into(),
        };
        self.overlays.push(Overlay::NameInput(NameInput {
            title,
            input: TextInput::default(),
            error: None,
            purpose: NamePurpose::NewFolder { parent },
            buttons: Default::default(),
        }));
    }

    /// `R`: rename the selected folder; on a profile, its form (the name is there).
    pub(super) fn open_rename(&mut self) {
        match self.explorer_row().map(|r| r.kind) {
            Some(explorer::RowKind::Folder(f)) => {
                self.overlays.push(Overlay::NameInput(NameInput {
                    title: Msg::NameRenameFolder { name: f.to_string() },
                    input: TextInput::new(f.name()),
                    error: None,
                    purpose: NamePurpose::RenameFolder(f),
                    buttons: Default::default(),
                }));
            }
            Some(explorer::RowKind::Profile(id)) => {
                let i = self.profiles.iter().position(|p| p.id == id);
                self.open_form(i, false);
            }
            Some(explorer::RowKind::Tunnel(id)) => self.open_tunnel_form(Some(id), false),
            Some(explorer::RowKind::Script(p)) => self.open_rename_script(p, false),
            Some(explorer::RowKind::ScriptFolder(p)) => self.open_rename_script(p, true),
            _ => {}
        }
    }

    /// Keys of the name input (`overlay.name_input`, text input).
    pub(super) fn name_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(n) = self.overlays.name_input_mut() else { return };
        match key.code {
            KeyCode::Esc => self.overlays.close(OverlayKind::NameInput),
            KeyCode::Enter if !repeat => self.name_entered(),
            KeyCode::Enter => {}
            _ => {
                if n.input.handle_key(&key) == InputResult::Changed {
                    n.error = None;
                }
            }
        }
    }

    /// `Enter` in the name input: create or rename the folder, or say why not.
    fn name_entered(&mut self) {
        let Some(n) = self.overlays.name_input() else { return };
        let typed = n.input.text().trim().to_string();
        let purpose = n.purpose.clone();
        // A preset's name is taken as typed (blanks around it are an error, not trimmed).
        let raw = n.input.text().to_string();
        let script = match &purpose {
            NamePurpose::SaveAsTunnel => Some(self.save_as_tunnel_named(&raw)),
            NamePurpose::NewScriptFolder { parent } => Some(self.new_script_folder(parent.clone(), &typed)),
            NamePurpose::RenameScript(from) => Some(self.rename_script(&from.clone(), &typed, false)),
            NamePurpose::RenameScriptFolder(from) => Some(self.rename_script(&from.clone(), &typed, true)),
            _ => None,
        };
        match script {
            Some(Ok(())) => {
                self.overlays.close(OverlayKind::NameInput);
                return;
            }
            Some(Err(label)) => {
                if let Some(n) = self.overlays.name_input_mut() {
                    n.error = Some(label);
                }
                return;
            }
            None => {}
        }
        let parent = match &purpose {
            NamePurpose::NewFolder { parent } => parent.clone(),
            NamePurpose::RenameFolder(f) => f.parent(),
            _ => return,
        };
        let path = match &parent {
            Some(p) => format!("{p}/{typed}"),
            None => typed.clone(),
        };
        let target = match FolderPath::parse(&path) {
            Ok(t) if self.folders.contains(&t) && purpose != NamePurpose::RenameFolder(t.clone()) => {
                Err(Label::ValidateFolderExists)
            }
            Ok(t) => Ok(t),
            Err(e) => Err(folder_error(&e)),
        };
        let target = match target {
            Ok(t) => t,
            Err(label) => {
                if let Some(n) = self.overlays.name_input_mut() {
                    n.error = Some(label);
                }
                return;
            }
        };
        self.overlays.close(OverlayKind::NameInput);
        let msg = match purpose {
            NamePurpose::NewFolder { parent } => {
                self.folders.insert(&target);
                if let Some(p) = parent {
                    for a in p.with_ancestors() {
                        if !self.folders.is_expanded(&a) {
                            self.folders.toggle(&a);
                        }
                    }
                }
                Msg::ExplorerFolderCreated { name: target.to_string() }
            }
            NamePurpose::RenameFolder(from) => {
                self.folders.rename(&from, &target);
                for p in self.profiles.iter_mut() {
                    if let Some(f) = p.folder_path().and_then(|f| f.rebase(&from, &target)) {
                        p.folder = Some(f.to_string());
                    }
                }
                Msg::ExplorerFolderRenamed { from: from.to_string(), to: target.to_string() }
            }
            _ => return,
        };
        let saved = self.persist();
        self.explorer.select_kind(explorer::RowKind::Folder(target));
        self.flash(saved.unwrap_or(Notice::new(msg, Level::Success)));
    }

    /// A paste in the explorer or on the welcome panel: a connection URL opens the new-profile
    /// form filled from it; anything else says what a paste does here.
    pub(super) fn paste_dsn(&mut self, text: &str) {
        let text = text.trim();
        if datarig_core::profile::dsn::parse(text).is_err() {
            return self.flash(Notice::new(Label::ExplorerPasteNotDsn, Level::Warning));
        }
        self.open_form(None, false);
        let profiles = &self.profiles;
        if let Some(f) = self.overlays.form_mut() {
            f.set_dsn(text, |n| profiles.iter().any(|p| p.name == n));
        }
    }
}
