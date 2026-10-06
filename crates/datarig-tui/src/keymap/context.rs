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
//! │  │  ├─ plan               (the Plan tab of the results pane)
//! │  │  ├─ welcome            (no profiles: the panel on the right)
//! │  │  ├─ editor.vim.normal  [editor]
//! │  │  │  └─ editor.ddl      [editor]   (a DDL tab's read-only text, vim Normal)
//! │  │  └─ editor.vim.visual  [editor]
//! │  ├─ editor.vim.insert     [editor][text]
//! │  ├─ editor.vim.search     [editor][text]   (the `/` and `?` prompt)
//! │  ├─ explorer.filter       [text]
//! │  └─ overlay.cell_viewer
//! └─ overlay.*              (which-key, help, help filter, command line, quick connect,
//!                            profile form, settings, chooser, chooser filter, name input,
//!                            saved-queries tree and its name field, action menu, password,
//!                            confirm, run confirm, completion)
//! ```

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
    Plan,
    Welcome,
    VimNormal,
    VimVisual,
    VimInsert,
    VimSearch,
    Ddl,
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
    pub const ALL: [Ctx; 34] = [
        Ctx::Root,
        Ctx::Workspace,
        Ctx::Nav,
        Ctx::Explorer,
        Ctx::ExplorerFilter,
        Ctx::Grid,
        Ctx::Inspector,
        Ctx::Plan,
        Ctx::Welcome,
        Ctx::VimNormal,
        Ctx::VimVisual,
        Ctx::VimInsert,
        Ctx::VimSearch,
        Ctx::Ddl,
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
            Ctx::Plan => "plan",
            Ctx::Welcome => "welcome",
            Ctx::VimNormal => "editor.vim.normal",
            Ctx::VimVisual => "editor.vim.visual",
            Ctx::VimInsert => "editor.vim.insert",
            Ctx::VimSearch => "editor.vim.search",
            Ctx::Ddl => "editor.ddl",
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
            Ctx::Nav | Ctx::VimInsert | Ctx::VimSearch | Ctx::ExplorerFilter | Ctx::CellViewer => Some(Ctx::Workspace),
            Ctx::Explorer | Ctx::Grid | Ctx::Inspector | Ctx::Plan | Ctx::Welcome | Ctx::VimNormal | Ctx::VimVisual => {
                Some(Ctx::Nav)
            }
            Ctx::Ddl => Some(Ctx::VimNormal),
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
                | Ctx::VimSearch
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
                | Ctx::ContextMenu
        )
    }

    /// The query editor: its keys may hide the keys of its ancestors (except protected keys).
    pub fn is_editor(self) -> bool {
        matches!(self, Ctx::VimNormal | Ctx::VimVisual | Ctx::VimInsert | Ctx::VimSearch | Ctx::Ddl)
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
            Ctx::Plan => {
                "The Plan tab of the results pane (an `EXPLAIN (FORMAT JSON)` result): `j`/`k` select a node, `h`/`l` close and open its children, `Enter` shows its detail, `v`/`V` and the digits pick a view, `y`/`Y` copy the plan as text or JSON."
            }
            Ctx::Welcome => "The welcome panel shown while there is no connection profile.",
            Ctx::VimNormal => "Query editor, vim Normal mode (`i` starts typing).",
            Ctx::VimVisual => "Query editor, vim Visual mode (`v` by character, `V` by line, `Ctrl+V` by block).",
            Ctx::VimInsert => "Query editor, vim Insert mode: typing (`Esc` goes back to Normal).",
            Ctx::VimSearch => {
                "The search prompt of `/` and `?` on the editor's last line: `Enter` searches, `Esc` goes back to where the cursor was."
            }
            Ctx::Ddl => {
                "A DDL tab in vim Normal mode: its text is read-only (moving, selecting, searching and yanking work; edits are refused); `r` reads it again, `o` opens it in a new console."
            }
            Ctx::Commands => {
                "The `:` command line: commands with arguments, and a search over every action; a click runs an entry, the wheel moves the selection."
            }
            Ctx::QuickConnect => {
                "Quick connect: a fuzzy list of the connection profiles; a click picks a row (on `▸` it lists the databases), the wheel selects, the pointer only highlights."
            }
            Ctx::Chooser => {
                "A list to pick from: a profile's color, icon or folder; a click picks a row, the wheel selects, the pointer only highlights."
            }
            Ctx::ChooserFilter => "The `/` filter of a list to pick from.",
            Ctx::NameInput => "A name to type: a new or renamed folder; OK and Cancel take a click.",
            Ctx::ScriptTree => "The folder tree of the saved queries (save as, open): the tree has the keyboard.",
            Ctx::ScriptTreeName => "The folder tree of the saved queries: its name field (save as) or filter (open).",
            Ctx::ContextMenu => {
                "The action menu: a right click (an explorer node, the grid, a tab, the editor) opens it at the pointer; `Space Space`, `Shift+F10` or the Menu key next to the selection, `Space t m` under the active tab. The selection's actions come first, then the pane's, each with its key in that pane. Typing filters it (when nothing in it matches, every action is searched); the arrows, `Ctrl+N`/`Ctrl+P` and `Tab` move, `Enter` runs, `Esc` closes (`Backspace` on an empty filter does not); a shown key that is not a character (`Ctrl+…`, `F…`) runs its item; the pointer selects the item under it."
            }
            Ctx::ProfileForm => {
                "Connection profile form. With the mouse: a click focuses a field (in a text, the cursor goes there), picks a value, steps a `‹ value ›` (its arrows; the value opens a list), switches the section or presses a button."
            }
            Ctx::Settings => {
                "The settings screen: one list by category, each setting with its description; a click selects a row and its `‹`/`›` change the value, the wheel selects, the pointer only highlights."
            }
            Ctx::Password => "Password prompt; a click on the field, the checkbox, OK or Cancel.",
            Ctx::Confirm => {
                "Yes/no confirmation: `y` does it; Enter keeps what the answer would lose (quit, delete, disconnect, close, switch, replace); for a copy it says yes. A button acts on its release (pressed and released on it, not right after the question appeared) and does what its key does; the pointer only underlines it (Enter stays on the safe one)."
            }
            Ctx::RunConfirm => {
                "Before statements that may do harm run: each is listed; Cancel has the focus, `y` or Enter on Run runs them, as a click on Run does (the pointer does not move the focus)."
            }
            Ctx::IconsAsk => {
                "Whether the terminal shows the Nerd Font icons (asked once, with a preview); No has the focus; a click on Yes or No answers."
            }
            Ctx::Busy => {
                "A notice that waits for background work (the launch-time move of passwords to the keychain); `Ctrl+C` quits there too."
            }
            Ctx::CellViewer => "Cell value viewer.",
            Ctx::Completion => "Completion popup in the editor (keys it does not use go to the editor).",
            Ctx::WhichKey => "Which-key popup of an unfinished leader sequence (other keys continue the sequence).",
            Ctx::Help => {
                "Keyboard help: one list of every context; the sections of the context it was opened from come first, open; the pointer selects the row under it."
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
            Ctx::Plan => Label::KeyctxPlan,
            Ctx::Welcome => Label::KeyctxWelcome,
            Ctx::VimNormal => Label::KeyctxEditorVimNormal,
            Ctx::VimVisual => Label::KeyctxEditorVimVisual,
            Ctx::VimInsert => Label::KeyctxEditorVimInsert,
            Ctx::VimSearch => Label::KeyctxEditorVimSearch,
            Ctx::Ddl => Label::KeyctxEditorDdl,
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
