//! Dialogs shown on top of the workspace, kept in one stack. The last entry has the keyboard
//! and is drawn last. Entries are kept in a fixed rank order (cell viewer < profile form < settings <
//! export dialog <
//! keyboard help < which-key < quick connect < tab list < context menu < saved-queries tree < name input <
//! chooser < picker <
//! confirmation < run confirmation < transaction summary < session view < icons question < busy notice < password prompt < command line), so a dialog that opens later but ranks lower —
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
use datarig_core::i18n::{Label, Localized, Msg};
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
    /// Delete this environment.
    DeleteEnvironment(datarig_core::policy::EnvironmentId),
    /// Delete this session preset.
    DeleteSession(datarig_core::profile::session::SessionPresetId),
    /// Run an `EXPLAIN ANALYZE` again to view its text plan as a plan (the statement waits in
    /// `App::pending_as_plan`). Cancel is the default.
    ExplainAgain,
    /// Reach the macOS keychain through `/usr/bin/security`
    /// (`[secrets] macos_keychain_access = "security_tool"`): any program running as the user can
    /// then read the passwords through it. Cancel is the default.
    KeychainSecurityTool,
    /// The one-time offer to stop the macOS keychain prompts after updates by switching to
    /// `/usr/bin/security` (`App::keychain_offer_key`): `y` switches, `n` declines for good,
    /// `Esc`/`Enter` decide later.
    KeychainOffer,
    /// Close the open form although it has changes that are not saved (its `Esc` or Cancel).
    /// Keeping on editing is the default.
    DiscardForm,
    /// Start an export that runs its statement again (or reads in a prod-like environment):
    /// the tab's export waits for the answer. Cancel is the default.
    ExportStart,
    /// Replace the file an export would write. Keeping it is the default.
    ExportReplace,
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
    /// The answer ends open transactions that can be ended first: `r` rolls them back, `c`
    /// commits them, then the answer goes on (only once the server confirmed each).
    pub tx: Option<TxChoices>,
    pub buttons: Buttons,
}

/// What a question whose answer ends open transactions offers besides yes and no
/// ([`Confirm::tx`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxChoices {
    /// `c` too: none of them failed (a failed one can only be rolled back).
    pub commit: bool,
    /// The transactions it offers to end: each tab and its transaction (`tx_epoch`). One that
    /// ended meanwhile (its session closed for being idle, the server ended it) stops the
    /// answer: nothing is said to be committed or rolled back that was not.
    pub ends: Vec<(super::TabId, u64)>,
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
            ConfirmAction::KeychainOffer => {
                let buttons = vec![
                    (Label::DialogButtonKeepAsking, KeyCode::Char('n')),
                    (Label::DialogButtonYesSwitch, KeyCode::Char('y')),
                ];
                return (buttons, None);
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
            | ConfirmAction::DeleteTunnel(_)
            | ConfirmAction::DeleteEnvironment(_)
            | ConfirmAction::DeleteSession(_) => (Label::DialogButtonCancel, Label::DialogButtonDelete),
            ConfirmAction::SetConnection(_) | ConfirmAction::SetContext(_) => {
                (Label::DialogButtonKeep, Label::DialogButtonSwitch)
            }
            ConfirmAction::OverwriteScript => (Label::DialogButtonKeep, Label::DialogButtonReplace),
            ConfirmAction::Copy => (Label::DialogButtonCancel, Label::DialogButtonCopy),
            ConfirmAction::FetchThenCopy => (Label::DialogButtonCancel, Label::DialogButtonFetchCopy),
            ConfirmAction::TrustHostKey => (Label::DialogButtonCancel, Label::DialogButtonTrust),
            ConfirmAction::ExplainAgain => (Label::DialogButtonCancel, Label::SafetyConfirmRun),
            ConfirmAction::KeychainSecurityTool => (Label::DialogButtonCancel, Label::DialogButtonTurnOn),
            ConfirmAction::ExportStart => (Label::DialogButtonCancel, Label::DialogButtonExport),
            ConfirmAction::ExportReplace => (Label::DialogButtonKeep, Label::DialogButtonReplace),
            ConfirmAction::DiscardForm => (Label::DialogButtonKeepEditing, Label::DialogButtonDiscard),
        };
        // Roll back first, commit first, then go on anyway (the server rolls back); Enter stays.
        if let Some(tx) = &self.tx {
            let mut b = vec![(no, KeyCode::Char('n')), (Label::TxButtonRollback, KeyCode::Char('r'))];
            if tx.commit {
                b.push((Label::TxButtonCommit, KeyCode::Char('c')));
            }
            b.push((yes, KeyCode::Char('y')));
            return (b, Some(0));
        }
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
    /// The statements run as they are, outside any transaction (a procedure that commits by
    /// itself): only on the tab's session `generation`, still open.
    pub as_is: bool,
    pub generation: u64,
    pub buttons: Buttons,
}

/// What tab `tab`'s open transaction did (`Space x s`, `:tx`): its statements that may have
/// written, oldest first, with their rows and time, how long it has been open and idle. `c`
/// commits, `r` rolls back, `Esc` (or `q`, `Enter`) closes; `j`/`k` scroll.
pub struct TxSummary {
    pub tab: super::TabId,
    /// The first line of the list on screen.
    pub scroll: usize,
    pub buttons: Buttons,
}

/// What a profile's query sessions start with and where each item comes from (`:session`, or
/// the profile form's key for its unsaved values), as it was when it opened: it does not follow
/// later changes. `Esc` (or `q`, `Enter`) closes; `j`/`k` scroll.
pub struct SessionView {
    pub title: Localized,
    /// The profile, its database and environment, whether that one is read-only.
    pub head: Vec<SessionLine>,
    /// The items of its plan, in order (or why it has none).
    pub lines: Vec<SessionLine>,
    pub foot: SessionLine,
    /// The first line of the list on screen, and the last first line it can be (as drawn).
    pub scroll: usize,
    pub max_scroll: usize,
    pub buttons: Buttons,
}

/// A line of the session view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionLine {
    /// Words of their own (wrapped), in a tone.
    Text(String, Tone),
    /// An app limit: its name, its value and where it comes from.
    Intent { what: String, value: String, source: String },
    /// Where the statements below come from.
    Source(String),
    /// A statement, sent as written.
    Statement(String),
    /// How the statement above goes behind a pooler in transaction mode.
    Pooled(datarig_core::profile::session::OnPooler),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Strong,
    Muted,
    Warning,
    Error,
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
    /// The export dialog.
    Export(Box<super::export_form::ExportForm>),
    /// Keyboard help (`Space ?`, `F1`).
    Help(Help),
    /// Keys that may follow an unfinished leader sequence.
    WhichKey(WhichKey),
    /// Fuzzy profile switcher (`Ctrl+O`).
    QuickConnect(QuickConnect),
    /// The open and recently closed tabs (`Space t t`, `:tabs`).
    TabList(super::tab_list::TabList),
    /// A right-click menu: the explorer's actions for a node, or the result grid's.
    ContextMenu(ContextMenu),
    /// The folder tree of the saved queries: "save as" and "open".
    ScriptTree(super::script_tree::ScriptTree),
    /// A name to type (a new or renamed folder).
    NameInput(NameInput),
    /// A list to pick from (a profile's color, icon or folder).
    Chooser(Chooser),
    /// A list to pick from with a typed filter (the profile form's SSH tunnel).
    Picker(super::picker::Picker),
    /// A yes/no question.
    Confirm(Confirm),
    /// Run statements that may do harm?
    RunConfirm(RunConfirm),
    /// What the open transaction did.
    TxSummary(TxSummary),
    /// What a profile's sessions start with.
    SessionView(SessionView),
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
    Export,
    Help,
    WhichKey,
    QuickConnect,
    TabList,
    ContextMenu,
    ScriptTree,
    NameInput,
    Chooser,
    Picker,
    Confirm,
    RunConfirm,
    TxSummary,
    SessionView,
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
            Overlay::Export(_) => OverlayKind::Export,
            Overlay::Help(_) => OverlayKind::Help,
            Overlay::WhichKey(_) => OverlayKind::WhichKey,
            Overlay::QuickConnect(_) => OverlayKind::QuickConnect,
            Overlay::TabList(_) => OverlayKind::TabList,
            Overlay::ContextMenu(_) => OverlayKind::ContextMenu,
            Overlay::ScriptTree(_) => OverlayKind::ScriptTree,
            Overlay::NameInput(_) => OverlayKind::NameInput,
            Overlay::Chooser(_) => OverlayKind::Chooser,
            Overlay::Picker(_) => OverlayKind::Picker,
            Overlay::Confirm(_) => OverlayKind::Confirm,
            Overlay::RunConfirm(_) => OverlayKind::RunConfirm,
            Overlay::TxSummary(_) => OverlayKind::TxSummary,
            Overlay::SessionView(_) => OverlayKind::SessionView,
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
    /// Open `o`, replacing an open overlay of the same kind. A form shown for the first time
    /// keeps its values to tell later whether it was edited.
    pub fn push(&mut self, mut o: Overlay) {
        if let Overlay::ProfileForm(f) = &mut o {
            f.mark_opened();
        }
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
            Some(Overlay::TxSummary(s)) => s.buttons.press = Press::default(),
            Some(Overlay::SessionView(s)) => s.buttons.press = Press::default(),
            Some(Overlay::IconsAsk(q)) => q.buttons.press = Press::default(),
            Some(Overlay::Password(p)) => p.buttons.press = Press::default(),
            Some(Overlay::NameInput(n)) => n.buttons.press = Press::default(),
            Some(Overlay::ProfileForm(f)) => f.press = Press::default(),
            Some(Overlay::Export(f)) => f.buttons.press = Press::default(),
            Some(Overlay::Chooser(c)) => c.press = Press::default(),
            Some(Overlay::Picker(p)) => p.rearm(),
            Some(Overlay::QuickConnect(q)) => q.press = Press::default(),
            Some(Overlay::TabList(l)) => l.press = Press::default(),
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

    /// The profile form: the open form, or the one waiting under it (a tunnel form opened from
    /// its SSH tunnel list).
    pub fn profile_form_mut(&mut self) -> Option<&mut ProfileForm> {
        let f = self.form_mut()?;
        if f.back_to.is_some() { f.back_to.as_deref_mut() } else { Some(f) }
    }

    /// Close the profile form and hand it over.
    pub fn take_form(&mut self) -> Option<Box<ProfileForm>> {
        let i = self.0.iter().position(|o| o.kind() == OverlayKind::ProfileForm)?;
        match self.0.remove(i) {
            Overlay::ProfileForm(f) => Some(f),
            _ => None,
        }
    }

    pub fn settings(&self) -> Option<&super::settings::SettingsScreen> {
        self.0.iter().find_map(|o| if let Overlay::Settings(s) = o { Some(s) } else { None })
    }

    pub fn export_form(&self) -> Option<&super::export_form::ExportForm> {
        self.0.iter().find_map(|o| if let Overlay::Export(f) = o { Some(&**f) } else { None })
    }

    pub fn export_form_mut(&mut self) -> Option<&mut super::export_form::ExportForm> {
        self.0.iter_mut().find_map(|o| if let Overlay::Export(f) = o { Some(&mut **f) } else { None })
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

    pub fn picker(&self) -> Option<&super::picker::Picker> {
        self.0.iter().find_map(|o| if let Overlay::Picker(p) = o { Some(p) } else { None })
    }

    pub fn picker_mut(&mut self) -> Option<&mut super::picker::Picker> {
        self.0.iter_mut().find_map(|o| if let Overlay::Picker(p) = o { Some(p) } else { None })
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

    pub fn tab_list(&self) -> Option<&super::tab_list::TabList> {
        self.0.iter().find_map(|o| if let Overlay::TabList(l) = o { Some(l) } else { None })
    }

    pub fn tab_list_mut(&mut self) -> Option<&mut super::tab_list::TabList> {
        self.0.iter_mut().find_map(|o| if let Overlay::TabList(l) = o { Some(l) } else { None })
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

    pub fn tx_summary(&self) -> Option<&TxSummary> {
        self.0.iter().find_map(|o| if let Overlay::TxSummary(s) = o { Some(s) } else { None })
    }

    pub fn tx_summary_mut(&mut self) -> Option<&mut TxSummary> {
        self.0.iter_mut().find_map(|o| if let Overlay::TxSummary(s) = o { Some(s) } else { None })
    }

    pub fn session_view(&self) -> Option<&SessionView> {
        self.0.iter().find_map(|o| if let Overlay::SessionView(s) = o { Some(s) } else { None })
    }

    pub fn session_view_mut(&mut self) -> Option<&mut SessionView> {
        self.0.iter_mut().find_map(|o| if let Overlay::SessionView(s) = o { Some(s) } else { None })
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
