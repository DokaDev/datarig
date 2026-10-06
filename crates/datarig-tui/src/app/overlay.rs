//! Dialogs shown on top of the workspace, kept in one stack. The last entry has the keyboard
//! and is drawn last. Entries are kept in a fixed rank order (cell viewer < profile form < settings <
//! keyboard help < which-key < quick connect < context menu < saved-queries tree < name input <
//! chooser <
//! confirmation < run confirmation < busy notice < password prompt < command line), so a dialog that opens later but ranks lower —
//! e.g. a password prompt raised by a connection event while the command line is open — goes
//! below.
//!
//! The completion popup is not here: it belongs to the editor.

use super::chooser::{Chooser, NameInput};
use super::guide::{Help, WhichKey};
use super::menu::ContextMenu;
use super::profiles::ProfileForm;
use super::quick::QuickConnect;
use super::{CommandLine, PasswordPrompt, Viewer};
use datarig_core::i18n::{Label, Msg};
use datarig_core::profile::ProfileId;
use datarig_core::profile::folder::FolderPath;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

/// How long a dialog's buttons and rows ignore presses after the dialog came on top: one that
/// appears under a clicking pointer (a question raised in the background or uncovered by a
/// dialog that closed, the second press of a double click) does not take that click.
pub const ARM_DELAY: Duration = Duration::from_millis(400);

/// What a press on a dialog armed: its buttons and list rows act as GUI buttons do, on the
/// release over the target that was pressed. The clock starts when the dialog is first drawn
/// on top; while another dialog covers it, it starts again (nothing pressed before counts).
#[derive(Clone, Debug)]
pub struct Press<T> {
    pub shown_at: Option<Instant>,
    pub armed: Option<T>,
}

impl<T> Default for Press<T> {
    fn default() -> Self {
        Press { shown_at: None, armed: None }
    }
}

impl<T: Copy + PartialEq> Press<T> {
    /// The dialog was drawn (`top`: on top, where the mouse reaches it).
    pub fn drawn(&mut self, top: bool, now: Instant) {
        if top {
            self.shown_at.get_or_insert(now);
        } else {
            *self = Press::default();
        }
    }

    /// A left press or release on `hit` at `now`: a press arms it (nothing in the first
    /// [`ARM_DELAY`] after the dialog came on top); the release over the armed target returns
    /// it, the target to act on. Anything else does nothing.
    pub fn press(&mut self, kind: MouseEventKind, hit: Option<T>, now: Instant) -> Option<T> {
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let ready = self.shown_at.is_some_and(|t| now.saturating_duration_since(t) >= ARM_DELAY);
                self.armed = hit.filter(|_| ready);
                None
            }
            MouseEventKind::Up(MouseButton::Left) => self.armed.take().filter(|a| hit == Some(*a)),
            _ => None,
        }
    }

    /// The pointer moved with no button held: a press is not in progress (its release was lost).
    pub fn disarm(&mut self) {
        self.armed = None;
    }
}

/// A dialog's buttons as last drawn (kept by the renderer, so a click hits what is on screen),
/// what a press armed and the one under the pointer. The pointer only highlights a button: the
/// focus, what `Enter` presses, stays where the keys put it.
#[derive(Clone, Debug, Default)]
pub struct Buttons {
    pub rects: Vec<Rect>,
    pub press: Press<usize>,
    pub hover: Option<usize>,
}

impl Buttons {
    /// The button at (x, y).
    pub fn at(&self, x: u16, y: u16) -> Option<usize> {
        self.rects.iter().position(|r| r.contains(Position::new(x, y)))
    }

    /// The pointer moved to (x, y): `true` when another button (or none) is under it now. A move
    /// has no button held: it drops an arm.
    pub fn hover(&mut self, x: u16, y: u16) -> bool {
        self.press.disarm();
        let h = self.at(x, y);
        std::mem::replace(&mut self.hover, h) != h
    }

    /// A left press or release at (x, y) at `now` ([`Press::press`]): the button to press.
    pub fn press(&mut self, kind: MouseEventKind, x: u16, y: u16, now: Instant) -> Option<usize> {
        let hit = self.at(x, y);
        self.press.press(kind, hit, now)
    }
}

/// What a yes/no confirmation does on "yes".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Quit although a query is running or a transaction is open.
    Quit,
    /// Save the profile form although its stored password moves or is removed.
    ChangeSource,
    /// Close a tab although a query runs or a transaction is open in it.
    CloseTab(super::TabId),
    /// Disconnect profile `id` (and connect again with `reconnect`) although a query runs or a
    /// transaction is open in one of its tabs.
    Disconnect { id: ProfileId, reconnect: bool },
    /// Delete profile `id` (its open tabs close).
    DeleteProfile(ProfileId),
    /// Delete an empty folder.
    DeleteFolder,
    /// Pick another connection for tab `tab` although a query runs or a transaction is open.
    SetConnection(super::TabId),
    /// Move tab `tab` to another database or schema although a query runs or a transaction is
    /// open: to `App::pending_context` when one was named (`:use`), else pick one.
    SetContext(super::TabId),
    /// Delete the saved query `Confirm::path` (its open tab closes).
    DeleteScript,
    /// Delete the empty folder `Confirm::path` of the saved queries.
    DeleteScriptFolder,
    /// The saved query of tab `tab` changed or went away on disk: `r` reloads it, `o`
    /// overwrites it with the tab's text, `Esc` decides later (nothing is written meanwhile).
    ScriptConflict(super::TabId),
    /// Copy many rows (the request waits in `App::pending_copy`).
    Copy,
    /// Fetch the rest of the active tab's rows, then copy them all.
    FetchThenCopy,
    /// Replace the saved query `Confirm::path` with the tab's text (the tree dialog of "save
    /// as"). Cancel is the default.
    OverwriteScript,
    /// Trust the host key an SSH tunnel asks about; written to datarig's
    /// known_hosts only. Cancel is the default.
    TrustHostKey,
    /// Delete this tunnel preset (and its saved secret).
    DeleteTunnel(datarig_core::profile::tunnel::TunnelId),
}

/// A yes/no question: `y` yes, `n`/`Esc` no; `Enter` no when the answer would lose something
/// (quit, delete, disconnect, close, switch, replace), yes to a copy.
pub struct Confirm {
    pub title: Label,
    pub text: Msg,
    /// More lines under the question, one message each (what else the answer does, per tab).
    pub details: Vec<Msg>,
    pub keys: Label,
    pub action: ConfirmAction,
    /// The folder `ConfirmAction::DeleteFolder` deletes.
    pub folder: Option<FolderPath>,
    /// The saved query or folder the action is about.
    pub path: Option<String>,
    pub buttons: Buttons,
}

impl Confirm {
    /// The buttons, the safe one first: each one's label and the key it presses, and the one
    /// `Enter` presses (none for the conflict question, where `Enter` does nothing).
    pub fn buttons(&self) -> (Vec<(Label, KeyCode)>, Option<usize>) {
        let (no, yes) = match self.action {
            ConfirmAction::ScriptConflict(_) if self.keys == Label::ScriptsConflictMissingKeys => {
                return (
                    vec![(Label::DialogButtonLater, KeyCode::Esc), (Label::DialogButtonSaveAgain, KeyCode::Char('o'))],
                    None,
                );
            }
            ConfirmAction::ScriptConflict(_) => {
                let buttons = vec![
                    (Label::DialogButtonLater, KeyCode::Esc),
                    (Label::DialogButtonReload, KeyCode::Char('r')),
                    (Label::DialogButtonOverwrite, KeyCode::Char('o')),
                ];
                return (buttons, None);
            }
            ConfirmAction::Quit => (Label::DialogButtonStay, Label::DialogButtonQuit),
            ConfirmAction::ChangeSource => (Label::DialogButtonCancel, Label::DialogButtonContinue),
            ConfirmAction::CloseTab(_) => (Label::DialogButtonKeep, Label::DialogButtonClose),
            ConfirmAction::Disconnect { .. } => (Label::DialogButtonStay, Label::DialogButtonDisconnect),
            ConfirmAction::DeleteProfile(_)
            | ConfirmAction::DeleteFolder
            | ConfirmAction::DeleteScript
            | ConfirmAction::DeleteScriptFolder
            | ConfirmAction::DeleteTunnel(_) => (Label::DialogButtonCancel, Label::DialogButtonDelete),
            ConfirmAction::SetConnection(_) | ConfirmAction::SetContext(_) => {
                (Label::DialogButtonKeep, Label::DialogButtonSwitch)
            }
            ConfirmAction::OverwriteScript => (Label::DialogButtonKeep, Label::DialogButtonReplace),
            ConfirmAction::Copy => (Label::DialogButtonCancel, Label::DialogButtonCopy),
            ConfirmAction::FetchThenCopy => (Label::DialogButtonCancel, Label::DialogButtonFetchCopy),
            ConfirmAction::TrustHostKey => (Label::DialogButtonCancel, Label::DialogButtonTrust),
        };
        // `Enter` says yes only to a copy (see `App::confirm_key`).
        let enter = if matches!(self.action, ConfirmAction::Copy | ConfirmAction::FetchThenCopy) { 1 } else { 0 };
        (vec![(no, KeyCode::Char('n')), (yes, KeyCode::Char('y'))], Some(enter))
    }
}

/// The question whether the terminal shows the Nerd Font icons, with a preview
/// of a few glyphs: asked once when the config has not decided (`icons = auto` or no key), or
/// again from the settings. **No has the focus**: `Enter` says yes only once "Yes" has it.
#[derive(Default)]
pub struct IconsAsk {
    /// "Yes" has the focus (it starts on "No").
    pub yes_focused: bool,
    pub buttons: Buttons,
}

/// The question before statements that may do harm run: every
/// such statement of the run is listed, and nothing runs until the user says so. **Cancel has
/// the focus**: `Enter` runs only once "Run" has it; `y` runs, `n`/`Esc` cancel.
pub struct RunConfirm {
    /// The tab, profile and tab generation the run was asked for: it runs only if they are
    /// still the same when confirmed.
    pub tab: super::TabId,
    pub profile: ProfileId,
    pub binding: u64,
    pub statements: Vec<String>,
    /// The statements that ask, in order.
    pub items: Vec<super::safety::Dangerous>,
    /// "Run" has the focus (it starts on "Cancel").
    pub run_focused: bool,
    pub buttons: Buttons,
}

/// A notice that waits for background work (moving passwords to the keychain). It has no keys
/// of its own: only the `root` keys work while it is shown.
pub struct Busy {
    pub title: Label,
    pub text: Label,
}

pub enum Overlay {
    /// Long cell text of the result grid.
    CellViewer(Viewer),
    /// The connection profile form.
    ProfileForm(Box<ProfileForm>),
    /// The settings screen.
    Settings(super::settings::SettingsScreen),
    /// Keyboard help (`Space ?`, `F1`).
    Help(Help),
    /// Keys that may follow an unfinished leader sequence.
    WhichKey(WhichKey),
    /// Fuzzy profile switcher (`Ctrl+O`).
    QuickConnect(QuickConnect),
    /// A right-click menu: the explorer's actions for a node, or the result grid's.
    ContextMenu(ContextMenu),
    /// The folder tree of the saved queries: "save as" and "open".
    ScriptTree(super::script_tree::ScriptTree),
    /// A name to type (a new or renamed folder).
    NameInput(NameInput),
    /// A list to pick from (a profile's color, icon or folder).
    Chooser(Chooser),
    /// A yes/no question.
    Confirm(Confirm),
    /// Run statements that may do harm?
    RunConfirm(RunConfirm),
    /// Do the Nerd Font icons show correctly? (asked once)
    IconsAsk(IconsAsk),
    /// Background work the user waits for.
    Busy(Busy),
    /// Password asked at connect time.
    Password(PasswordPrompt),
    /// The `:` command line.
    Commands(CommandLine),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverlayKind {
    CellViewer,
    ProfileForm,
    Settings,
    Help,
    WhichKey,
    QuickConnect,
    ContextMenu,
    ScriptTree,
    NameInput,
    Chooser,
    Confirm,
    RunConfirm,
    IconsAsk,
    Busy,
    Password,
    Commands,
}

impl Overlay {
    pub fn kind(&self) -> OverlayKind {
        match self {
            Overlay::CellViewer(_) => OverlayKind::CellViewer,
            Overlay::ProfileForm(_) => OverlayKind::ProfileForm,
            Overlay::Settings(_) => OverlayKind::Settings,
            Overlay::Help(_) => OverlayKind::Help,
            Overlay::WhichKey(_) => OverlayKind::WhichKey,
            Overlay::QuickConnect(_) => OverlayKind::QuickConnect,
            Overlay::ContextMenu(_) => OverlayKind::ContextMenu,
            Overlay::ScriptTree(_) => OverlayKind::ScriptTree,
            Overlay::NameInput(_) => OverlayKind::NameInput,
            Overlay::Chooser(_) => OverlayKind::Chooser,
            Overlay::Confirm(_) => OverlayKind::Confirm,
            Overlay::RunConfirm(_) => OverlayKind::RunConfirm,
            Overlay::IconsAsk(_) => OverlayKind::IconsAsk,
            Overlay::Busy(_) => OverlayKind::Busy,
            Overlay::Password(_) => OverlayKind::Password,
            Overlay::Commands(_) => OverlayKind::Commands,
        }
    }
}

#[derive(Default)]
pub struct Overlays(Vec<Overlay>);

impl Overlays {
    /// Open `o`, replacing an open overlay of the same kind.
    pub fn push(&mut self, o: Overlay) {
        let kind = o.kind();
        self.close(kind);
        let at = self.0.iter().position(|x| x.kind() > kind).unwrap_or(self.0.len());
        self.0.insert(at, o);
    }

    pub fn close(&mut self, kind: OverlayKind) {
        self.0.retain(|o| o.kind() != kind);
    }

    pub fn is_open(&self, kind: OverlayKind) -> bool {
        self.0.iter().any(|o| o.kind() == kind)
    }

    /// The overlay that has the keyboard.
    pub fn top(&self) -> Option<&Overlay> {
        self.0.last()
    }

    /// Bottom to top (drawing order).
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Overlay> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The top dialog's arming clock and arm start over (it just came on top).
    pub fn rearm_top(&mut self) {
        match self.0.last_mut() {
            Some(Overlay::Confirm(c)) => c.buttons.press = Press::default(),
            Some(Overlay::RunConfirm(c)) => c.buttons.press = Press::default(),
            Some(Overlay::IconsAsk(q)) => q.buttons.press = Press::default(),
            Some(Overlay::Password(p)) => p.buttons.press = Press::default(),
            Some(Overlay::NameInput(n)) => n.buttons.press = Press::default(),
            Some(Overlay::ProfileForm(f)) => f.press = Press::default(),
            Some(Overlay::Chooser(c)) => c.press = Press::default(),
            Some(Overlay::QuickConnect(q)) => q.press = Press::default(),
            Some(Overlay::Commands(c)) => c.press = Press::default(),
            _ => {}
        }
    }

    /// A dialog that blocks the screen below it (everything but the cell viewer).
    pub fn modal(&self) -> bool {
        self.0.iter().any(|o| o.kind() != OverlayKind::CellViewer)
    }

    pub fn command_line(&self) -> Option<&CommandLine> {
        self.0.iter().find_map(|o| if let Overlay::Commands(p) = o { Some(p) } else { None })
    }

    pub fn command_line_mut(&mut self) -> Option<&mut CommandLine> {
        self.0.iter_mut().find_map(|o| if let Overlay::Commands(p) = o { Some(p) } else { None })
    }

    pub fn form(&self) -> Option<&ProfileForm> {
        self.0.iter().find_map(|o| if let Overlay::ProfileForm(f) = o { Some(&**f) } else { None })
    }

    pub fn form_mut(&mut self) -> Option<&mut ProfileForm> {
        self.0.iter_mut().find_map(|o| if let Overlay::ProfileForm(f) = o { Some(&mut **f) } else { None })
    }

    pub fn settings(&self) -> Option<&super::settings::SettingsScreen> {
        self.0.iter().find_map(|o| if let Overlay::Settings(s) = o { Some(s) } else { None })
    }

    pub fn settings_mut(&mut self) -> Option<&mut super::settings::SettingsScreen> {
        self.0.iter_mut().find_map(|o| if let Overlay::Settings(s) = o { Some(s) } else { None })
    }

    pub fn menu(&self) -> Option<&ContextMenu> {
        self.0.iter().find_map(|o| if let Overlay::ContextMenu(m) = o { Some(m) } else { None })
    }

    pub fn menu_mut(&mut self) -> Option<&mut ContextMenu> {
        self.0.iter_mut().find_map(|o| if let Overlay::ContextMenu(m) = o { Some(m) } else { None })
    }

    pub fn chooser(&self) -> Option<&Chooser> {
        self.0.iter().find_map(|o| if let Overlay::Chooser(c) = o { Some(c) } else { None })
    }

    pub fn chooser_mut(&mut self) -> Option<&mut Chooser> {
        self.0.iter_mut().find_map(|o| if let Overlay::Chooser(c) = o { Some(c) } else { None })
    }

    pub fn script_tree(&self) -> Option<&super::script_tree::ScriptTree> {
        self.0.iter().find_map(|o| if let Overlay::ScriptTree(t) = o { Some(t) } else { None })
    }

    pub fn script_tree_mut(&mut self) -> Option<&mut super::script_tree::ScriptTree> {
        self.0.iter_mut().find_map(|o| if let Overlay::ScriptTree(t) = o { Some(t) } else { None })
    }

    pub fn name_input(&self) -> Option<&NameInput> {
        self.0.iter().find_map(|o| if let Overlay::NameInput(n) = o { Some(n) } else { None })
    }

    pub fn name_input_mut(&mut self) -> Option<&mut NameInput> {
        self.0.iter_mut().find_map(|o| if let Overlay::NameInput(n) = o { Some(n) } else { None })
    }

    pub fn quick(&self) -> Option<&QuickConnect> {
        self.0.iter().find_map(|o| if let Overlay::QuickConnect(q) = o { Some(q) } else { None })
    }

    pub fn quick_mut(&mut self) -> Option<&mut QuickConnect> {
        self.0.iter_mut().find_map(|o| if let Overlay::QuickConnect(q) = o { Some(q) } else { None })
    }

    pub fn prompt(&self) -> Option<&PasswordPrompt> {
        self.0.iter().find_map(|o| if let Overlay::Password(p) = o { Some(p) } else { None })
    }

    /// Close the password prompt and hand it over.
    pub fn take_prompt(&mut self) -> Option<PasswordPrompt> {
        let i = self.0.iter().position(|o| o.kind() == OverlayKind::Password)?;
        match self.0.remove(i) {
            Overlay::Password(p) => Some(p),
            _ => None,
        }
    }

    pub fn prompt_mut(&mut self) -> Option<&mut PasswordPrompt> {
        self.0.iter_mut().find_map(|o| if let Overlay::Password(p) = o { Some(p) } else { None })
    }

    pub fn help(&self) -> Option<&Help> {
        self.0.iter().find_map(|o| if let Overlay::Help(h) = o { Some(h) } else { None })
    }

    pub fn help_mut(&mut self) -> Option<&mut Help> {
        self.0.iter_mut().find_map(|o| if let Overlay::Help(h) = o { Some(h) } else { None })
    }

    pub fn which_key(&self) -> Option<&WhichKey> {
        self.0.iter().find_map(|o| if let Overlay::WhichKey(w) = o { Some(w) } else { None })
    }

    pub fn which_key_mut(&mut self) -> Option<&mut WhichKey> {
        self.0.iter_mut().find_map(|o| if let Overlay::WhichKey(w) = o { Some(w) } else { None })
    }

    pub fn confirm(&self) -> Option<&Confirm> {
        self.0.iter().find_map(|o| if let Overlay::Confirm(c) = o { Some(c) } else { None })
    }

    pub fn confirm_mut(&mut self) -> Option<&mut Confirm> {
        self.0.iter_mut().find_map(|o| if let Overlay::Confirm(c) = o { Some(c) } else { None })
    }

    pub fn run_confirm(&self) -> Option<&RunConfirm> {
        self.0.iter().find_map(|o| if let Overlay::RunConfirm(c) = o { Some(c) } else { None })
    }

    /// Close the run confirmation and hand it over.
    pub fn take_run_confirm(&mut self) -> Option<RunConfirm> {
        let i = self.0.iter().position(|o| o.kind() == OverlayKind::RunConfirm)?;
        match self.0.remove(i) {
            Overlay::RunConfirm(c) => Some(c),
            _ => None,
        }
    }

    pub fn icons_ask(&self) -> Option<&IconsAsk> {
        self.0.iter().find_map(|o| if let Overlay::IconsAsk(c) = o { Some(c) } else { None })
    }

    pub fn icons_ask_mut(&mut self) -> Option<&mut IconsAsk> {
        self.0.iter_mut().find_map(|o| if let Overlay::IconsAsk(c) = o { Some(c) } else { None })
    }

    pub fn run_confirm_mut(&mut self) -> Option<&mut RunConfirm> {
        self.0.iter_mut().find_map(|o| if let Overlay::RunConfirm(c) = o { Some(c) } else { None })
    }

    pub fn viewer(&self) -> Option<&Viewer> {
        self.0.iter().find_map(|o| if let Overlay::CellViewer(v) = o { Some(v) } else { None })
    }

    pub fn viewer_mut(&mut self) -> Option<&mut Viewer> {
        self.0.iter_mut().find_map(|o| if let Overlay::CellViewer(v) = o { Some(v) } else { None })
    }
}

#[cfg(test)]
mod tests;
