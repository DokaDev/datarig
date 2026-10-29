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
//! │  │  │  └─ grid.matches     (a search of the grid is on: `n`/`N` go to its matches)
//! │  │  ├─ inspector          (the result inspector panel, once clicked)
//! │  │  ├─ plan               (the Plan tab of the results pane)
//! │  │  ├─ welcome            (no profiles: the panel on the right)
//! │  │  ├─ editor.vim.normal  [editor]
//! │  │  │  └─ editor.ddl      [editor]   (a DDL tab's read-only text, vim Normal)
//! │  │  └─ editor.vim.visual  [editor]
//! │  ├─ editor.vim.insert     [editor][text]
//! │  ├─ editor.vim.search     [editor][text]   (the `/` and `?` prompt)
//! │  ├─ explorer.filter       [text]
//! │  ├─ grid.search           [text]   (the grid's `/` and `?` prompt)
//! │  └─ overlay.cell_viewer
//! └─ overlay.*              (which-key, help, help filter, command line, quick connect,
//!                            tab list, profile form, settings, export dialog, chooser, chooser filter, name input,
//!                            saved-queries tree and its name field, action menu, password,
//!                            confirm, run confirm, transaction summary, session view, completion)
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
    GridMatches,
    GridSearch,
    Inspector,
    Plan,
    Chart,
    Welcome,
    VimNormal,
    VimVisual,
    VimInsert,
    VimSearch,
    Ddl,
    Commands,
    QuickConnect,
    TabList,
    ProfileForm,
    Settings,
    Export,
    Chooser,
    ChooserFilter,
    Picker,
    NameInput,
    ScriptTree,
    ScriptTreeName,
    ContextMenu,
    Password,
    Confirm,
    RunConfirm,
    TxSummary,
    SessionView,
    IconsAsk,
    Busy,
    CellViewer,
    Completion,
    WhichKey,
    Help,
    HelpFilter,
}

impl Ctx {
    pub const ALL: [Ctx; 42] = [
        Ctx::Root,
        Ctx::Workspace,
        Ctx::Nav,
        Ctx::Explorer,
        Ctx::ExplorerFilter,
        Ctx::Grid,
        Ctx::GridMatches,
        Ctx::GridSearch,
        Ctx::Inspector,
        Ctx::Plan,
        Ctx::Chart,
        Ctx::Welcome,
        Ctx::VimNormal,
        Ctx::VimVisual,
        Ctx::VimInsert,
        Ctx::VimSearch,
        Ctx::Ddl,
        Ctx::Commands,
        Ctx::QuickConnect,
        Ctx::TabList,
        Ctx::ProfileForm,
        Ctx::Settings,
        Ctx::Export,
        Ctx::Chooser,
        Ctx::ChooserFilter,
        Ctx::Picker,
        Ctx::NameInput,
        Ctx::ScriptTree,
        Ctx::ScriptTreeName,
        Ctx::ContextMenu,
        Ctx::Password,
        Ctx::Confirm,
        Ctx::RunConfirm,
        Ctx::TxSummary,
        Ctx::SessionView,
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
            Ctx::GridMatches => "grid.matches",
            Ctx::GridSearch => "grid.search",
            Ctx::Inspector => "inspector",
            Ctx::Plan => "plan",
            Ctx::Chart => "chart",
            Ctx::Welcome => "welcome",
            Ctx::VimNormal => "editor.vim.normal",
            Ctx::VimVisual => "editor.vim.visual",
            Ctx::VimInsert => "editor.vim.insert",
            Ctx::VimSearch => "editor.vim.search",
            Ctx::Ddl => "editor.ddl",
            Ctx::Commands => "overlay.commands",
            Ctx::QuickConnect => "overlay.quick_connect",
            Ctx::TabList => "overlay.tab_list",
            Ctx::Chooser => "overlay.chooser",
            Ctx::ChooserFilter => "overlay.chooser.filter",
            Ctx::Picker => "overlay.picker",
            Ctx::NameInput => "overlay.name_input",
            Ctx::ScriptTree => "overlay.script_tree",
            Ctx::ScriptTreeName => "overlay.script_tree.name",
            Ctx::ContextMenu => "overlay.context_menu",
            Ctx::ProfileForm => "overlay.profile_form",
            Ctx::Settings => "overlay.settings",
            Ctx::Export => "overlay.export",
            Ctx::Password => "overlay.password",
            Ctx::Confirm => "overlay.confirm",
            Ctx::RunConfirm => "overlay.run_confirm",
            Ctx::TxSummary => "overlay.tx_summary",
            Ctx::SessionView => "overlay.session_view",
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
            Ctx::Nav | Ctx::VimInsert | Ctx::VimSearch | Ctx::ExplorerFilter | Ctx::GridSearch | Ctx::CellViewer => {
                Some(Ctx::Workspace)
            }
            Ctx::GridMatches => Some(Ctx::Grid),
            Ctx::Explorer
            | Ctx::Grid
            | Ctx::Inspector
            | Ctx::Plan
            | Ctx::Chart
            | Ctx::Welcome
            | Ctx::VimNormal
            | Ctx::VimVisual => Some(Ctx::Nav),
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
                | Ctx::GridSearch
                | Ctx::Commands
                | Ctx::QuickConnect
                | Ctx::TabList
                | Ctx::ProfileForm
                | Ctx::Export
                | Ctx::ChooserFilter
                | Ctx::Picker
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

    /// A mode of its parent: while it is on, its keys may hide the parent's (the grid while a
    /// search is on: `n` and `Esc` there are the search's, not the grid's).
    pub fn is_mode(self) -> bool {
        self == Ctx::GridMatches
    }

    /// A search prompt outside the editor: its reserved editing keys (`Ctrl+W`) win over the
    /// workspace's, as they do in the editor's own prompt.
    pub fn is_prompt(self) -> bool {
        self == Ctx::GridSearch
    }

    /// One line for `docs/keybindings.md`.
    pub fn describe(self) -> &'static str {
        match self {
            Ctx::Root => "Works everywhere, also while a dialog is open.",
            Ctx::Workspace => "The workspace without a dialog, including text input in the editor.",
            Ctx::Nav => "Every pane that is not text input (explorer, results, vim Normal/Visual).",
            Ctx::Explorer => "The explorer: folders, connection profiles, their databases and schema trees.",
            Ctx::ExplorerFilter => {
                "The `/` filter of the explorer: profiles (name, host, database), folders, tunnels, environments, Startup SQL (name, statements) and saved queries, and under connected profiles the databases, schemas, tables, views and structure items already loaded."
            }
            Ctx::Grid => {
                "The result grid. The wheel scrolls the rows; a sideways wheel (a trackpad) or Shift+wheel scrolls the view sideways, four characters a step, the columns at its edges partly shown, until a key or a click moves the cell (`h`/`l` bring its column into view, moving the view as little as it takes). `/` searches the fetched rows forward, `?` backward (a Rust regular expression, case sensitive unless it starts with `(?i)`; NULL never matches)."
            }
            Ctx::GridMatches => {
                "The result grid while a search is on (after `/` or `?`): `n` goes to the next matching cell in the search's direction, `N` the other way, from page to page through every fetched row (in the spill file too), around the ends with a warning; `Esc` ends the search, and `n` shows the next page again."
            }
            Ctx::GridSearch => {
                "The search prompt of `/` and `?` on the line under the grid: the cursor goes to the first matching cell while the pattern is typed, `Enter` searches (an empty pattern: the last one), `Esc` goes back to where the cursor was, `Ctrl+W` deletes the word before the cursor. The full values are searched as fetched (not the shown cut), cell by cell, row by row."
            }
            Ctx::Inspector => {
                "The result inspector next to the grid, after a click on it: `j`/`k` move between the columns and the grid's cell follows (on the Row tab, a click on a column moves it there too), `Enter` shows the value whole."
            }
            Ctx::Plan => {
                "The Plan tab of the results pane (a plan result: PostgreSQL's JSON, MySQL's tree): `j`/`k` select a node, `h`/`l` close and open its children, `Enter` shows its detail, `v`/`V` and the digits pick a view, `y` copies the plan as the raw view shows it, `Y` as the server sent it."
            }
            Ctx::Chart => {
                "The Chart tab of the results pane (`c` in the grid, or its tab in the strip; it is drawn only once viewed): the fetched rows of the shown result as bars or lines, nothing run again. The arrows and `h`/`j`/`k`/`l` move the cursor along the bars or points (the other direction picks a series), `Enter` shows the point's row in the grid, `v`/`V` and the digits pick the kind, `x`/`s`/`b` choose the X column, the values and a column that splits them into series, `S` a logarithmic scale, `y`/`Y` copy the numbers as TSV or the drawing as text; a click picks a point, a kind or a column choice."
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
            Ctx::TabList => {
                "The tab list (`Space t t`, `:tabs`, `:ls`, `:buffers`): the open tabs, the most recently active first, then the tabs closed in this run. Typing filters by name, connection, database, schema, kind or number; the arrows, `Ctrl+N`/`Ctrl+P` and `Tab` move, `Enter` goes to the tab (brings a closed one back), `Ctrl+D` closes the selected tab (asking as `Ctrl+W` does), `Esc` closes the list (`Backspace` on an empty filter does not). A click picks an entry, the wheel selects, the pointer only highlights."
            }
            Ctx::Chooser => {
                "A list to pick from: a profile's color, icon or folder, a chart's columns (the values' list stays open: `Enter` adds or removes one); a click picks a row, the wheel selects, the pointer only highlights."
            }
            Ctx::ChooserFilter => "The `/` filter of a list to pick from.",
            Ctx::Picker => {
                "A list to pick from with a typed filter: a field of the profile form (`Enter` on it) — its driver, SSL mode, password storage, bastion login and its secret's storage, SSH tunnel or environment — each value with what it means; the tunnel's and the environment's end with the entry that makes a new one (\"New SSH tunnel…\", \"New environment…\"), and a value that cannot be picked (a driver the app has none for, the keychain while it does not work) is struck through. Typing filters it as the action menu is filtered; the arrows, `Ctrl+N`/`Ctrl+P` and `Tab` move, `Enter` picks, `Esc` closes it and changes nothing. A click picks a row, the wheel selects, the pointer only highlights."
            }
            Ctx::NameInput => "A name to type: a new or renamed folder; OK and Cancel take a click.",
            Ctx::ScriptTree => {
                "The folder tree of the saved queries (save as, open), of the file system (the SSH key file, where an export goes) or of folders only (the folder chooser): the tree has the keyboard. `j`/`k` and the arrows move; `l`/`Right` opens the selected folder (one of the file system is read first); `h`/`Left` closes an open folder, else selects the folder the row is in. In a file system tree, `h`/`Left` on `../` or on the folder shown (and `l`/`Right` on `../`) goes up to the parent of the folder shown. `Enter`: save as saves into the selected folder, the folder chooser picks it; the other trees open or close a folder, go up on `../`, and pick a file (the saved query to open, the key file, the file the export replaces). In the export tree the top row (`Enter`) and `Ctrl+S` (any folder) save there, and `.` shows hidden names; `n` makes a folder (save as, folder chooser, export). `Tab` (and `/` in the folder chooser and the export tree) gives the keyboard to the name or filter field; `Esc` closes (`q` too in the folder chooser and the export tree). The mouse: a click selects a row, one on a folder's arrow opens or closes it; a second click on the selected row does what `Enter` does there, except in save as, where it only opens or closes a folder, and in the key file tree, where a path typed in the field is taken only by `Enter`; the wheel scrolls."
            }
            Ctx::ScriptTreeName => {
                "The folder tree of the saved queries: its name field (save as) or filter (open); of the folder chooser: its filter; of the file system: its filter, where a text with `/` or a leading `~` is a path `Enter` goes to. While the field is empty, `Left`/`Right` on a folder or file act as in the tree: they close or open the folder, `Left` selects the folder the row is in."
            }
            Ctx::ContextMenu => {
                "The action menu: a right click (an explorer node, the grid, a tab, the editor) opens it at the pointer; `Space Space`, `Shift+F10` or the Menu key next to the selection, `Space t m` under the active tab. The selection's actions come first, then the pane's, each with its key in that pane. Typing filters it (when nothing in it matches, every action is searched); the arrows, `Ctrl+N`/`Ctrl+P` and `Tab` move, `Enter` runs, `Esc` closes (`Backspace` on an empty filter does not); a shown key that is not a character (`Ctrl+…`, `F…`) runs its item; the pointer selects the item under it."
            }
            Ctx::ProfileForm => {
                "Connection profile form (also the SSH tunnel, environment, default rules and Startup SQL forms). `Esc` or Cancel closes it; with changes not saved it asks first (`Enter` keeps editing, `y` discards), and so does quitting the app while it is open (`y` discards and the quit goes on). With the mouse: a click focuses a field (in a text, the cursor goes there), picks a value, opens the list of a `value ▾`, steps a `‹ value ›` (its arrows; the value opens a list), switches the section or presses a button. A Startup SQL list (the preset form's statements, a profile's own startup SQL) has one statement per row: `Enter` after a filled row adds one below, on an empty row removes it and moves on; `↑`/`↓` go from row to row and leave the list at its first and last; `Tab` leaves it; `Backspace` on an empty row and `Ctrl+D` remove a row; a paste of several statements makes a row of each. With the mouse: a click on a row puts the cursor there, `×` removes it, \"+ Add statement\" adds one at the end (both on the release), the wheel scrolls a list longer than its room."
            }
            Ctx::Settings => {
                "The settings screen: one list by category, each setting with its description; a click selects a row and its `‹`/`›` change the value, the wheel selects, the pointer only highlights."
            }
            Ctx::Export => {
                "The export dialog (`results.export`, `:export` alone): the rows (every row, the rows fetched, the selection while there is one), the format, CSV's options and the file. `Tab`/`Shift+Tab` and the arrows move between the fields, `Enter` or `Space` on a value lists what it can be (`←`/`→` step through them), `Space`, `Enter` or `←`/`→` tick the header line, `Enter` in a text field or `Enter`/`Space` on Export exports (on Cancel: closes), `Esc` cancels. Under the rows it says how they will be read, under the format what it cannot do here (a format that comes later, an INSERT or UPDATE the result's keys do not allow, struck through in the list). In the file's field `Tab` first completes a name (again: lists the names, then puts each in turn); `[Browse…]` picks where the file goes in a tree. A click focuses a field, puts the cursor in a text, opens a value's list or ticks the checkbox; Export and Cancel act on the release."
            }
            Ctx::Password => "Password prompt; a click on the field, the checkbox, OK or Cancel.",
            Ctx::Confirm => {
                "Yes/no confirmation: `y` does it; Enter keeps what the answer would lose (quit, delete, disconnect, close, switch, replace, discard); for a copy it says yes. A button acts on its release (pressed and released on it, not right after the question appeared) and does what its key does; the pointer only lights its background (Enter stays on the safe one)."
            }
            Ctx::RunConfirm => {
                "Before statements that may do harm run: each is listed; Cancel has the focus, `y` or Enter on Run runs them, as a click on Run does (the pointer does not move the focus)."
            }
            Ctx::TxSummary => {
                "What the open transaction did (its statements that may have written, rows, time; how long it has been open and idle): `c` commits, `r` rolls back, `Esc` closes; a click on a button does what its key does."
            }
            Ctx::SessionView => {
                "What a profile's query sessions start with (`:session` or `:startup`; in the profile form `Ctrl+E`, for its unsaved values), in the order they are applied, each with where it comes from: the app's limits, the Startup SQL's statements, the profile's own; behind a pooler in transaction mode, how each PostgreSQL statement goes there. `j`/`k` scroll, `Esc` closes; a click on Close closes too."
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
            Ctx::GridMatches => Label::KeyctxGridMatches,
            Ctx::GridSearch => Label::KeyctxGridSearch,
            Ctx::Inspector => Label::KeyctxInspector,
            Ctx::Plan => Label::KeyctxPlan,
            Ctx::Chart => Label::KeyctxChart,
            Ctx::Welcome => Label::KeyctxWelcome,
            Ctx::VimNormal => Label::KeyctxEditorVimNormal,
            Ctx::VimVisual => Label::KeyctxEditorVimVisual,
            Ctx::VimInsert => Label::KeyctxEditorVimInsert,
            Ctx::VimSearch => Label::KeyctxEditorVimSearch,
            Ctx::Ddl => Label::KeyctxEditorDdl,
            Ctx::Commands => Label::KeyctxOverlayCommands,
            Ctx::QuickConnect => Label::KeyctxOverlayQuickConnect,
            Ctx::TabList => Label::KeyctxOverlayTabList,
            Ctx::Chooser => Label::KeyctxOverlayChooser,
            Ctx::ChooserFilter => Label::KeyctxOverlayChooserFilter,
            Ctx::Picker => Label::KeyctxOverlayPicker,
            Ctx::NameInput => Label::KeyctxOverlayNameInput,
            Ctx::ScriptTree => Label::KeyctxOverlayScriptTree,
            Ctx::ScriptTreeName => Label::KeyctxOverlayScriptTreeName,
            Ctx::ContextMenu => Label::KeyctxOverlayContextMenu,
            Ctx::ProfileForm => Label::KeyctxOverlayProfileForm,
            Ctx::Settings => Label::KeyctxOverlaySettings,
            Ctx::Export => Label::KeyctxOverlayExport,
            Ctx::Password => Label::KeyctxOverlayPassword,
            Ctx::Confirm => Label::KeyctxOverlayConfirm,
            Ctx::RunConfirm => Label::KeyctxOverlayRunConfirm,
            Ctx::TxSummary => Label::KeyctxOverlayTxSummary,
            Ctx::SessionView => Label::KeyctxOverlaySessionView,
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
