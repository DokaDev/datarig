//! Key contexts. Keys resolve from the current context
//! outwards to `root`. Dialogs (`overlay.*`) hang off `root` directly, so while one is open
//! only the `root` keys pass through. The cell viewer is not modal: it hangs off `workspace`,
//! so run, cancel, the command line and the other workspace keys keep working while it is open.
//!
//! ```text
//! root
//! ├─ workspace
//! │  ├─ nav                 (non-text panes: leader, `:`, Tab)
//! │  │  ├─ explorer
//! │  │  ├─ grid
//! │  │  ├─ inspector          (the result inspector panel, once clicked)
//! │  │  ├─ welcome            (no profiles: the panel on the right)
//! │  │  ├─ editor.vim.normal  [editor]
//! │  │  └─ editor.vim.visual  [editor]
//! │  ├─ editor.vim.insert     [editor][text]
//! │  ├─ editor.standard       [editor][text]
//! │  ├─ explorer.filter       [text]
//! │  └─ overlay.cell_viewer
//! └─ overlay.*              (which-key, help, help filter, command line, quick connect,
//!                            profile form, settings, chooser, chooser filter, name input,
//!                            saved-queries tree and its name field, context menu, password,
//!                            confirm, run confirm, completion)
//! ```

use datarig_core::config::EditorMode;
use datarig_core::i18n::Label;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ctx {
    Root,
    Workspace,
    Nav,
    Explorer,
    ExplorerFilter,
    Grid,
    Inspector,
    Welcome,
    VimNormal,
    VimVisual,
    VimInsert,
    Standard,
    Commands,
    QuickConnect,
    ProfileForm,
    Settings,
    Chooser,
    ChooserFilter,
    NameInput,
    ScriptTree,
    ScriptTreeName,
    ContextMenu,
    Password,
    Confirm,
    RunConfirm,
    IconsAsk,
    Busy,
    CellViewer,
    Completion,
    WhichKey,
    Help,
    HelpFilter,
}

impl Ctx {
    pub const ALL: [Ctx; 32] = [
        Ctx::Root,
        Ctx::Workspace,
        Ctx::Nav,
        Ctx::Explorer,
        Ctx::ExplorerFilter,
        Ctx::Grid,
        Ctx::Inspector,
        Ctx::Welcome,
        Ctx::VimNormal,
        Ctx::VimVisual,
        Ctx::VimInsert,
        Ctx::Standard,
        Ctx::Commands,
        Ctx::QuickConnect,
        Ctx::ProfileForm,
        Ctx::Settings,
        Ctx::Chooser,
        Ctx::ChooserFilter,
        Ctx::NameInput,
        Ctx::ScriptTree,
        Ctx::ScriptTreeName,
        Ctx::ContextMenu,
        Ctx::Password,
        Ctx::Confirm,
        Ctx::RunConfirm,
        Ctx::IconsAsk,
        Ctx::Busy,
        Ctx::CellViewer,
        Ctx::Completion,
        Ctx::WhichKey,
        Ctx::Help,
        Ctx::HelpFilter,
    ];

    /// Name used in `[keymap.<name>]` and the docs.
    pub fn name(self) -> &'static str {
        match self {
            Ctx::Root => "root",
            Ctx::Workspace => "workspace",
            Ctx::Nav => "nav",
            Ctx::Explorer => "explorer",
            Ctx::ExplorerFilter => "explorer.filter",
            Ctx::Grid => "grid",
            Ctx::Inspector => "inspector",
            Ctx::Welcome => "welcome",
            Ctx::VimNormal => "editor.vim.normal",
            Ctx::VimVisual => "editor.vim.visual",
            Ctx::VimInsert => "editor.vim.insert",
            Ctx::Standard => "editor.standard",
            Ctx::Commands => "overlay.commands",
            Ctx::QuickConnect => "overlay.quick_connect",
            Ctx::Chooser => "overlay.chooser",
            Ctx::ChooserFilter => "overlay.chooser.filter",
            Ctx::NameInput => "overlay.name_input",
            Ctx::ScriptTree => "overlay.script_tree",
            Ctx::ScriptTreeName => "overlay.script_tree.name",
            Ctx::ContextMenu => "overlay.context_menu",
            Ctx::ProfileForm => "overlay.profile_form",
            Ctx::Settings => "overlay.settings",
            Ctx::Password => "overlay.password",
            Ctx::Confirm => "overlay.confirm",
            Ctx::RunConfirm => "overlay.run_confirm",
            Ctx::IconsAsk => "overlay.icons_ask",
            Ctx::Busy => "overlay.busy",
            Ctx::CellViewer => "overlay.cell_viewer",
            Ctx::Completion => "overlay.completion",
            Ctx::WhichKey => "overlay.which_key",
            Ctx::Help => "overlay.help",
            Ctx::HelpFilter => "overlay.help.filter",
        }
    }

    pub fn from_name(s: &str) -> Option<Ctx> {
        Ctx::ALL.into_iter().find(|c| c.name() == s)
    }

    pub fn parent(self) -> Option<Ctx> {
        match self {
            Ctx::Root => None,
            Ctx::Workspace => Some(Ctx::Root),
            Ctx::Nav | Ctx::VimInsert | Ctx::Standard | Ctx::ExplorerFilter | Ctx::CellViewer => Some(Ctx::Workspace),
            Ctx::Explorer | Ctx::Grid | Ctx::Inspector | Ctx::Welcome | Ctx::VimNormal | Ctx::VimVisual => {
                Some(Ctx::Nav)
            }
            _ => Some(Ctx::Root),
        }
    }

    /// This context and its ancestors, innermost first.
    pub fn chain(self) -> Vec<Ctx> {
        std::iter::successors(Some(self), |c| c.parent()).collect()
    }

    /// Text input: letters, digits, symbols and Space are typed; no leader, no Hangul mapping,
    /// and only non-character keys may be bound.
    pub fn is_text_input(self) -> bool {
        matches!(
            self,
            Ctx::VimInsert
                | Ctx::Standard
                | Ctx::ExplorerFilter
                | Ctx::Commands
                | Ctx::QuickConnect
                | Ctx::ProfileForm
                | Ctx::ChooserFilter
                | Ctx::NameInput
                | Ctx::ScriptTreeName
                | Ctx::Password
                | Ctx::Completion
                | Ctx::HelpFilter
        )
    }

    /// The query editor: its keys may hide the keys of its ancestors (except protected keys).
    pub fn is_editor(self) -> bool {
        matches!(self, Ctx::VimNormal | Ctx::VimVisual | Ctx::VimInsert | Ctx::Standard)
    }

    /// The contexts that can be current with editor mode `mode`.
    pub fn for_mode(mode: EditorMode) -> Vec<Ctx> {
        Ctx::ALL
            .into_iter()
            .filter(|c| match mode {
                EditorMode::Vim => *c != Ctx::Standard,
                EditorMode::Standard => !matches!(c, Ctx::VimNormal | Ctx::VimVisual | Ctx::VimInsert),
            })
            .collect()
    }

    /// One line for `docs/keybindings.md`.
    pub fn describe(self) -> &'static str {
        match self {
            Ctx::Root => "Works everywhere, also while a dialog is open.",
            Ctx::Workspace => "The workspace without a dialog, including text input in the editor.",
            Ctx::Nav => "Every pane that is not text input (explorer, results, vim Normal/Visual).",
            Ctx::Explorer => "The explorer: folders, connection profiles, their databases and schema trees.",
            Ctx::ExplorerFilter => "The `/` filter of the explorer (profile names).",
            Ctx::Grid => "The result grid.",
            Ctx::Inspector => "The result inspector next to the grid, after a click on it.",
            Ctx::Welcome => "The welcome panel shown while there is no connection profile.",
            Ctx::VimNormal => "Query editor, `[editor] mode = \"vim\"`, Normal mode.",
            Ctx::VimVisual => "Query editor, `[editor] mode = \"vim\"`, Visual mode.",
            Ctx::VimInsert => "Query editor, `[editor] mode = \"vim\"`, Insert mode.",
            Ctx::Standard => "Query editor, `[editor] mode = \"standard\"` (its editing keys are not implemented yet).",
            Ctx::Commands => "The `:` command line: commands with arguments, and a search over every action.",
            Ctx::QuickConnect => "Quick connect: a fuzzy list of the connection profiles.",
            Ctx::Chooser => "A list to pick from: a profile's color, icon or folder.",
            Ctx::ChooserFilter => "The `/` filter of a list to pick from.",
            Ctx::NameInput => "A name to type: a new or renamed folder.",
            Ctx::ScriptTree => "The folder tree of the saved queries (save as, open): the tree has the keyboard.",
            Ctx::ScriptTreeName => "The folder tree of the saved queries: its name field (save as) or filter (open).",
            Ctx::ContextMenu => {
                "A right-click menu (the explorer's node, the result grid); the key shown next to an item runs it too."
            }
            Ctx::ProfileForm => "Connection profile form.",
            Ctx::Settings => "The settings screen: one list by category, each setting with its description.",
            Ctx::Password => "Password prompt.",
            Ctx::Confirm => {
                "Yes/no confirmation: `y` does it; Enter keeps what the answer would lose (quit, delete, disconnect, close, switch, replace); for a copy it says yes."
            }
            Ctx::RunConfirm => {
                "Before statements that may do harm run: each is listed; Cancel has the focus, `y` or Enter on Run runs them."
            }
            Ctx::IconsAsk => {
                "Whether the terminal shows the Nerd Font icons (asked once, with a preview); No has the focus."
            }
            Ctx::Busy => {
                "A notice that waits for background work (the launch-time move of passwords to the keychain); `Ctrl+C` quits there too."
            }
            Ctx::CellViewer => "Cell value viewer.",
            Ctx::Completion => "Completion popup in the editor (keys it does not use go to the editor).",
            Ctx::WhichKey => "Which-key popup of an unfinished leader sequence (other keys continue the sequence).",
            Ctx::Help => {
                "Keyboard help: one list of every context; the sections of the context it was opened from come first, open."
            }
            Ctx::HelpFilter => "The `/` filter of the keyboard help.",
        }
    }

    /// The context's title (keyboard help sections).
    pub fn title(self) -> Label {
        match self {
            Ctx::Root => Label::KeyctxRoot,
            Ctx::Workspace => Label::KeyctxWorkspace,
            Ctx::Nav => Label::KeyctxNav,
            Ctx::Explorer => Label::KeyctxExplorer,
            Ctx::ExplorerFilter => Label::KeyctxExplorerFilter,
            Ctx::Grid => Label::KeyctxGrid,
            Ctx::Inspector => Label::KeyctxInspector,
            Ctx::Welcome => Label::KeyctxWelcome,
            Ctx::VimNormal => Label::KeyctxEditorVimNormal,
            Ctx::VimVisual => Label::KeyctxEditorVimVisual,
            Ctx::VimInsert => Label::KeyctxEditorVimInsert,
            Ctx::Standard => Label::KeyctxEditorStandard,
            Ctx::Commands => Label::KeyctxOverlayCommands,
            Ctx::QuickConnect => Label::KeyctxOverlayQuickConnect,
            Ctx::Chooser => Label::KeyctxOverlayChooser,
            Ctx::ChooserFilter => Label::KeyctxOverlayChooserFilter,
            Ctx::NameInput => Label::KeyctxOverlayNameInput,
            Ctx::ScriptTree => Label::KeyctxOverlayScriptTree,
            Ctx::ScriptTreeName => Label::KeyctxOverlayScriptTreeName,
            Ctx::ContextMenu => Label::KeyctxOverlayContextMenu,
            Ctx::ProfileForm => Label::KeyctxOverlayProfileForm,
            Ctx::Settings => Label::KeyctxOverlaySettings,
            Ctx::Password => Label::KeyctxOverlayPassword,
            Ctx::Confirm => Label::KeyctxOverlayConfirm,
            Ctx::RunConfirm => Label::KeyctxOverlayRunConfirm,
            Ctx::IconsAsk => Label::KeyctxOverlayIconsAsk,
            Ctx::Busy => Label::KeyctxOverlayBusy,
            Ctx::CellViewer => Label::KeyctxOverlayCellViewer,
            Ctx::Completion => Label::KeyctxOverlayCompletion,
            Ctx::WhichKey => Label::KeyctxOverlayWhichKey,
            Ctx::Help => Label::KeyctxOverlayHelp,
            Ctx::HelpFilter => Label::KeyctxOverlayHelpFilter,
        }
    }
}
