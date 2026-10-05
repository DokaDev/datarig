//! Named actions. Key bindings (`crate::keymap`) and the
//! `:` command line both resolve to an [`Action`] and go through `App::dispatch`, so a key and
//! its command-line entry can never drift apart.

use super::copy::{CopyFormat, CopyScope};
use super::overlay::OverlayKind;
use super::{App, Focus};
use datarig_core::config::IconsSetting;
use datarig_core::i18n::{I18n, Label, Lang};
use datarig_core::secret::{DefaultSource, SourceKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LangSetting {
    Auto,
    En,
    Ko,
}

impl LangSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            LangSetting::Auto => "auto",
            LangSetting::En => "en",
            LangSetting::Ko => "ko",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "en" => LangSetting::En,
            "ko" => LangSetting::Ko,
            _ => LangSetting::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    RunStatement,
    CancelQuery,
    OpenCommands,
    ShowCompletions,
    /// The active tab's text in the user's editor (`$VISUAL`, `$EDITOR`).
    ExternalEdit,
    /// Stop until the shell's `fg` (Unix job control).
    Suspend,
    /// Format the statement under the cursor or the selection (`:format`).
    FormatSql,
    /// Comment lines out or back in, as `gc`.
    ToggleComment,
    NewProfile,
    /// Test the selected profile (explorer) or the active tab's profile.
    TestConnection,
    /// Test the active tab's profile.
    TestCurrent,
    /// Fuzzy profile switcher: go to the profile's tab, or connect and open a console.
    QuickConnect,
    /// Edit / duplicate the selected profile.
    EditProfile,
    DuplicateProfile,
    /// Edit the active tab's profile.
    EditCurrent,
    /// Disconnect the selected profile (its tabs stay).
    Disconnect,
    /// Disconnect / reconnect the active tab's profile.
    DisconnectCurrent,
    ReconnectCurrent,
    /// A new console tab on the selected profile.
    OpenConsole,
    /// Pick another connection for the active tab.
    SetTabConnection,
    /// Pick the active tab's database and schema.
    SetTabContext,
    SetLanguage(LangSetting),
    FocusNext,
    FocusPrev,
    Quit,
    /// `icons = on | off | auto`.
    SetIcons(IconsSetting),
    /// Icons on <-> off (from what is shown now).
    ToggleIcons,
    /// `[secrets] default_source`: the password storage a new profile starts with.
    SetDefaultSource(DefaultSource),
    /// Keyboard help, the current context's sections open.
    Help,
    /// Keyboard help with every section open.
    HelpAll,
    /// Result grid -> editor.
    PaneBack,
    /// A new console tab on the current connection.
    NewTab,
    /// Close the active tab (asks first when a query runs or a transaction is open).
    CloseTab,
    /// Bring the most recently closed tab back.
    ReopenTab,
    NextTab,
    PrevTab,
    /// Tab 1..9.
    GotoTab(u8),
    /// Save the active tab: a console asks for a name, a saved query is written.
    ScriptSave,
    /// Save the active tab under a new name (the tab shows the new file).
    ScriptSaveAs,
    /// Rename the active tab's saved query.
    ScriptRename,
    /// Pick a saved query to open.
    ScriptOpen,
    /// Delete the active tab's saved query (asks first).
    ScriptDelete,
    /// The profile form: pick the SSH key file in a tree of the file system.
    PickKeyFile,
    /// A new tunnel preset (the tunnel form).
    NewTunnel,
    /// The profile form: its own tunnel becomes a tunnel preset when the form is saved.
    SaveAsTunnel,
    /// The settings screen.
    OpenSettings,
    /// Show or hide the result detail (the inspector panel, or the status bar preview).
    ToggleDetail,
    /// Copy the selection (else the cell) or every fetched row, in a format.
    Copy(super::copy::CopyScope, super::copy::CopyFormat),
    /// The inspector's other tab (Cell <-> Row).
    DetailTab,
    Explorer(ExplorerAction),
    Grid(GridAction),
    /// The results pane of a query tab.
    Panel(PanelAction),
    /// The next (`true`) or previous result tab of a query tab: the row results of the last
    /// run, then its Messages.
    ResultTab(bool),
    /// The next page of the shown result: fetched, or run again past a closed portal.
    PageNext,
    /// The previous page of the shown result.
    PagePrev,
    /// Count every row of the shown result (`SELECT count(*)`, sent for the user).
    CountRows,
}

/// What the results pane of a query tab does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Hide it (the editor takes the height) or show it again.
    Toggle,
    /// The results take the whole height, or back.
    Maximize,
    /// Taller by a step.
    Grow,
    /// Shorter by a step.
    Shrink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerAction {
    Down,
    Up,
    Expand,
    Collapse,
    Activate,
    Top,
    Bottom,
    Refresh,
    /// `/`: type a filter over the profile names.
    Filter,
    FilterClear,
    FilterAccept,
    /// `Esc`: cancel a connection attempt, else clear the filter.
    Back,
    /// Delete the selected profile or empty folder (asks first).
    Delete,
    /// Move the selected profile to a folder (a list).
    Move,
    /// A new folder in the selected folder.
    NewFolder,
    /// Rename the selected folder (a profile: its form).
    Rename,
    /// The actions of the selected node, as a menu (right click).
    ContextMenu,
    /// A new console in the selected schema (or with the selected profile's defaults).
    ConsoleHere,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridAction {
    Left,
    Right,
    Down,
    Up,
    PageDown,
    PageUp,
    HalfDown,
    HalfUp,
    Top,
    Bottom,
    FirstCol,
    LastCol,
    ViewCell,
    /// `y`: the cell (a selected range as TSV).
    CopyCell,
    /// `Y`: the row (the selected rows) as TSV.
    CopyRow,
    /// `v`: start or drop a rectangular selection.
    Select,
    /// `V`: start or drop a selection of whole rows.
    SelectRows,
}

pub struct ActionSpec {
    pub action: Action,
    /// Stable identifier (command-line search, `[keymap.*]` config, docs).
    pub id: &'static str,
    /// Label (command line, which-key, help, docs).
    pub label: Label,
    /// Whether the action can run now. The command line lists only these; keys of other actions do
    /// nothing.
    pub when: fn(&App) -> bool,
    /// Terminal auto-repeat (a held key) runs it again: movement yes, commands no.
    pub repeatable: bool,
}

fn anywhere(_: &App) -> bool {
    true
}

/// The platform can suspend the program (Unix).
fn can_suspend(_: &App) -> bool {
    super::effects::SUSPEND_SUPPORTED
}

/// There are profiles, so the tabs and the editor are shown (otherwise the welcome panel).
fn in_workspace(a: &App) -> bool {
    !a.profiles.is_empty()
}

/// There is a tab (the workspace may have none: its empty state is shown then).
fn has_tab(a: &App) -> bool {
    in_workspace(a) && !a.tabs.is_empty()
}

/// The active tab is a query tab (a console or a saved query), not a table tab: it has a
/// text to save and a connection it can switch.
fn query_tab(a: &App) -> bool {
    has_tab(a) && !a.tab().is_table()
}

/// The active tab is a query tab on a profile (its database and schema can change).
fn query_tab_profile(a: &App) -> bool {
    query_tab(a) && a.tab().profile.is_some()
}

/// The active tab runs on a profile.
fn tab_profile(a: &App) -> bool {
    has_tab(a) && a.tab().profile.is_some()
}

/// Pane focus can move: the panes are drawn, no cell viewer sits on top of them, and there is
/// another pane (the welcome panel, or a tab's editor and results).
fn panes(a: &App) -> bool {
    !a.layout.too_small && !a.overlays.is_open(OverlayKind::CellViewer) && (a.profiles.is_empty() || !a.tabs.is_empty())
}

/// The keyboard help would open below a password prompt, so not while one is open.
fn help_available(a: &App) -> bool {
    !a.overlays.is_open(OverlayKind::Password)
}

fn explorer_focused(a: &App) -> bool {
    a.focus == Focus::Tree
}

fn explorer_filtering(a: &App) -> bool {
    a.focus == Focus::Tree && a.explorer.filtering
}

/// The explorer has the focus and its cursor is on a profile (or below it).
fn explorer_profile(a: &App) -> bool {
    explorer_focused(a) && a.selected_profile().is_some()
}

/// The explorer has the focus and its cursor is on a profile or a tunnel preset.
fn explorer_profile_or_tunnel(a: &App) -> bool {
    explorer_focused(a) && (a.selected_profile().is_some() || a.selected_tunnel().is_some())
}

/// There is a profile to act on (the explorer's selection, else the active tab's), or the
/// explorer's cursor is on a tunnel preset.
fn action_profile(a: &App) -> bool {
    a.action_profile().is_some() || (explorer_focused(a) && a.selected_tunnel().is_some())
}

/// The profile form's own tunnel can become a tunnel preset.
fn own_tunnel_field(a: &App) -> bool {
    a.config_writable()
        && a.overlays
            .form()
            .is_some_and(|f| !f.is_tunnel() && f.section == super::profiles::Section::Ssh && f.ssh_enabled && !f.saving)
}

/// Tunnel presets can be made: the config file can be written.
fn presets_writable(a: &App) -> bool {
    a.tunnels_shown()
}

/// Saved queries can be kept (there is a data directory).
/// The profile form shows the SSH key file field.
fn key_file_field(a: &App) -> bool {
    a.overlays.form().is_some_and(|f| {
        f.section == super::profiles::Section::Ssh && f.ssh_fields().contains(&super::profiles::Field::SshKeyFile)
    })
}

fn scripts(a: &App) -> bool {
    in_workspace(a) && a.scripts_available()
}

/// The active tab is a saved query.
fn tab_script(a: &App) -> bool {
    scripts(a) && a.tab().script().is_some()
}

fn grid_focused(a: &App) -> bool {
    in_workspace(a) && a.focus == Focus::Results
}

/// The grid or the inspector next to it has the focus.
fn results_focused(a: &App) -> bool {
    in_workspace(a) && matches!(a.focus, Focus::Results | Focus::Inspector)
}

/// The active tab shows a result with rows (something to copy), not its Messages.
fn has_rows(a: &App) -> bool {
    has_tab(a)
        && a.tab().exec.view == super::tabs::ResultView::Rows
        && matches!(&a.tab().results, super::Results::Rows(rs) if !rs.rows.is_empty())
}

/// The active tab shows a row result (a grid to page through).
fn rows_shown(a: &App) -> bool {
    has_tab(a)
        && a.results_shown()
        && a.tab().exec.view == super::tabs::ResultView::Rows
        && matches!(&a.tab().results, super::Results::Rows(_))
}

/// The inspector panel is shown.
fn detail_panel(a: &App) -> bool {
    in_workspace(a) && a.inspector_shown()
}

/// The active tab is a query tab (console or saved query) that ran something: its results
/// pane can be hidden or shown.
fn pane_known(a: &App) -> bool {
    has_tab(a) && !a.tab().is_table() && a.tab().ran
}

/// The active query tab shows its results pane.
fn pane_shown(a: &App) -> bool {
    has_tab(a) && !a.tab().is_table() && a.results_shown()
}

/// The results pane of a query tab has more than one result tab (the Messages of its last run
/// count) to switch between.
fn result_tabs(a: &App) -> bool {
    pane_shown(a) && a.tab().ran && !a.tab().result_tabs().is_empty()
}

/// The pane is shown and not maximised: its height can change.
fn pane_sized(a: &App) -> bool {
    pane_shown(a) && !a.tab().pane.maximized
}

const fn act(action: Action, id: &'static str, label: Label, when: fn(&App) -> bool) -> ActionSpec {
    ActionSpec { action, id, label, when, repeatable: false }
}

/// Movement: repeats while the key is held.
const fn mv(action: Action, id: &'static str, label: Label, when: fn(&App) -> bool) -> ActionSpec {
    ActionSpec { action, id, label, when, repeatable: true }
}

use ExplorerAction as E;
use GridAction as G;

/// Every action, in command-line order.
pub const REGISTRY: &[ActionSpec] = &[
    act(Action::RunStatement, "query.execute_current", Label::ActionQueryExecuteCurrent, has_tab),
    act(Action::CancelQuery, "query.cancel", Label::ActionQueryCancel, has_tab),
    act(Action::ShowCompletions, "editor.complete", Label::ActionEditorComplete, has_tab),
    act(Action::ExternalEdit, "editor.open_external", Label::ActionEditorOpenExternal, has_tab),
    act(Action::FormatSql, "editor.format", Label::ActionEditorFormat, query_tab),
    act(Action::ToggleComment, "editor.comment_toggle", Label::ActionEditorCommentToggle, query_tab),
    act(Action::QuickConnect, "conn.quick_connect", Label::ActionConnQuickConnect, in_workspace),
    act(Action::NewProfile, "conn.new", Label::ActionConnNew, anywhere),
    act(Action::EditProfile, "conn.edit", Label::ActionConnEdit, explorer_profile_or_tunnel),
    act(Action::DuplicateProfile, "conn.duplicate", Label::ActionConnDuplicate, explorer_profile_or_tunnel),
    act(Action::NewTunnel, "tunnel.new", Label::ActionTunnelNew, presets_writable),
    act(Action::TestConnection, "conn.test", Label::ActionConnTest, action_profile),
    act(Action::Disconnect, "conn.disconnect", Label::ActionConnDisconnect, explorer_profile),
    act(Action::OpenConsole, "conn.open_console", Label::ActionConnOpenConsole, explorer_profile),
    act(Action::EditCurrent, "conn.edit_current", Label::ActionConnEditCurrent, tab_profile),
    act(Action::TestCurrent, "conn.test_current", Label::ActionConnTestCurrent, tab_profile),
    act(Action::DisconnectCurrent, "conn.disconnect_current", Label::ActionConnDisconnectCurrent, tab_profile),
    act(Action::ReconnectCurrent, "conn.reconnect_current", Label::ActionConnReconnectCurrent, tab_profile),
    act(Action::SetTabConnection, "tab.set_connection", Label::ActionTabSetConnection, query_tab),
    act(Action::SetTabContext, "tab.set_context", Label::ActionTabSetContext, query_tab_profile),
    act(Action::SetLanguage(LangSetting::En), "ui.language.en", Label::ActionUiLanguageEn, anywhere),
    act(Action::SetLanguage(LangSetting::Ko), "ui.language.ko", Label::ActionUiLanguageKo, anywhere),
    act(Action::SetLanguage(LangSetting::Auto), "ui.language.auto", Label::ActionUiLanguageAuto, anywhere),
    act(Action::FocusNext, "pane.next", Label::ActionPaneNext, panes),
    act(Action::FocusPrev, "pane.prev", Label::ActionPanePrev, panes),
    act(Action::OpenCommands, "commands.open", Label::ActionCommandsOpen, anywhere),
    act(Action::Quit, "app.quit", Label::ActionAppQuit, anywhere),
    act(Action::Suspend, "app.suspend", Label::ActionAppSuspend, can_suspend),
    act(Action::OpenSettings, "settings.open", Label::ActionSettingsOpen, anywhere),
    act(Action::ToggleIcons, "ui.icons.toggle", Label::ActionUiIconsToggle, anywhere),
    act(Action::SetIcons(IconsSetting::On), "ui.icons.on", Label::ActionUiIconsOn, anywhere),
    act(Action::SetIcons(IconsSetting::Off), "ui.icons.off", Label::ActionUiIconsOff, anywhere),
    act(Action::SetIcons(IconsSetting::Auto), "ui.icons.auto", Label::ActionUiIconsAuto, anywhere),
    act(
        Action::SetDefaultSource(DefaultSource::Auto),
        "secrets.default.auto",
        Label::ActionSecretsDefaultAuto,
        anywhere,
    ),
    act(
        Action::SetDefaultSource(DefaultSource::Kind(SourceKind::Keychain)),
        "secrets.default.keychain",
        Label::ActionSecretsDefaultKeychain,
        anywhere,
    ),
    act(
        Action::SetDefaultSource(DefaultSource::Kind(SourceKind::File)),
        "secrets.default.file",
        Label::ActionSecretsDefaultFile,
        anywhere,
    ),
    act(
        Action::SetDefaultSource(DefaultSource::Kind(SourceKind::Command)),
        "secrets.default.command",
        Label::ActionSecretsDefaultCommand,
        anywhere,
    ),
    act(
        Action::SetDefaultSource(DefaultSource::Kind(SourceKind::Env)),
        "secrets.default.env",
        Label::ActionSecretsDefaultEnv,
        anywhere,
    ),
    act(
        Action::SetDefaultSource(DefaultSource::Kind(SourceKind::Prompt)),
        "secrets.default.prompt",
        Label::ActionSecretsDefaultPrompt,
        anywhere,
    ),
    act(Action::NewTab, "tab.new_console", Label::ActionTabNewConsole, in_workspace),
    act(Action::CloseTab, "tab.close", Label::ActionTabClose, has_tab),
    act(Action::ReopenTab, "tab.reopen_closed", Label::ActionTabReopenClosed, in_workspace),
    mv(Action::NextTab, "tab.next", Label::ActionTabNext, has_tab),
    mv(Action::PrevTab, "tab.prev", Label::ActionTabPrev, has_tab),
    act(Action::GotoTab(1), "tab.goto.1", Label::ActionTabGoto1, has_tab),
    act(Action::GotoTab(2), "tab.goto.2", Label::ActionTabGoto2, has_tab),
    act(Action::GotoTab(3), "tab.goto.3", Label::ActionTabGoto3, has_tab),
    act(Action::GotoTab(4), "tab.goto.4", Label::ActionTabGoto4, has_tab),
    act(Action::GotoTab(5), "tab.goto.5", Label::ActionTabGoto5, has_tab),
    act(Action::GotoTab(6), "tab.goto.6", Label::ActionTabGoto6, has_tab),
    act(Action::GotoTab(7), "tab.goto.7", Label::ActionTabGoto7, has_tab),
    act(Action::GotoTab(8), "tab.goto.8", Label::ActionTabGoto8, has_tab),
    act(Action::GotoTab(9), "tab.goto.9", Label::ActionTabGoto9, has_tab),
    // Without a data directory these say why (they are the first thing a user tries).
    act(Action::ScriptSave, "script.save", Label::ActionScriptSave, query_tab),
    act(Action::ScriptSaveAs, "script.save_as", Label::ActionScriptSaveAs, query_tab),
    act(Action::ScriptRename, "script.rename", Label::ActionScriptRename, tab_script),
    act(Action::ScriptOpen, "script.open", Label::ActionScriptOpen, scripts),
    act(Action::ScriptDelete, "script.delete", Label::ActionScriptDelete, tab_script),
    act(Action::PickKeyFile, "form.pick_key_file", Label::ActionFormPickKeyFile, key_file_field),
    act(Action::SaveAsTunnel, "form.save_as_tunnel", Label::ActionFormSaveAsTunnel, own_tunnel_field),
    act(Action::Help, "help.context", Label::ActionHelpContext, help_available),
    act(Action::HelpAll, "help.all", Label::ActionHelpAll, help_available),
    act(Action::PaneBack, "pane.back", Label::ActionPaneBack, results_focused),
    mv(Action::Explorer(E::Down), "explorer.down", Label::ActionExplorerDown, explorer_focused),
    mv(Action::Explorer(E::Up), "explorer.up", Label::ActionExplorerUp, explorer_focused),
    act(Action::Explorer(E::Expand), "explorer.expand", Label::ActionExplorerExpand, explorer_focused),
    act(Action::Explorer(E::Collapse), "explorer.collapse", Label::ActionExplorerCollapse, explorer_focused),
    act(Action::Explorer(E::Activate), "explorer.activate", Label::ActionExplorerActivate, explorer_focused),
    act(Action::Explorer(E::Top), "explorer.top", Label::ActionExplorerTop, explorer_focused),
    act(Action::Explorer(E::Bottom), "explorer.bottom", Label::ActionExplorerBottom, explorer_focused),
    act(Action::Explorer(E::Refresh), "explorer.refresh", Label::ActionExplorerRefresh, explorer_focused),
    act(Action::Explorer(E::Filter), "explorer.filter", Label::ActionExplorerFilter, explorer_focused),
    act(
        Action::Explorer(E::FilterClear),
        "explorer.filter_clear",
        Label::ActionExplorerFilterClear,
        explorer_filtering,
    ),
    act(
        Action::Explorer(E::FilterAccept),
        "explorer.filter_accept",
        Label::ActionExplorerFilterAccept,
        explorer_filtering,
    ),
    act(Action::Explorer(E::Back), "explorer.back", Label::ActionExplorerBack, explorer_focused),
    act(Action::Explorer(E::Delete), "explorer.delete", Label::ActionExplorerDelete, explorer_focused),
    act(Action::Explorer(E::Move), "explorer.move", Label::ActionExplorerMove, explorer_focused),
    act(Action::Explorer(E::NewFolder), "folder.new", Label::ActionFolderNew, explorer_focused),
    act(Action::Explorer(E::Rename), "explorer.rename", Label::ActionExplorerRename, explorer_focused),
    act(Action::Explorer(E::ContextMenu), "explorer.context_menu", Label::ActionExplorerContextMenu, explorer_focused),
    act(
        Action::Explorer(E::ConsoleHere),
        "explorer.new_console_here",
        Label::ActionExplorerNewConsoleHere,
        explorer_profile,
    ),
    mv(Action::Grid(G::Left), "grid.left", Label::ActionGridLeft, grid_focused),
    mv(Action::Grid(G::Right), "grid.right", Label::ActionGridRight, grid_focused),
    mv(Action::Grid(G::Down), "grid.down", Label::ActionGridDown, grid_focused),
    mv(Action::Grid(G::Up), "grid.up", Label::ActionGridUp, grid_focused),
    mv(Action::Grid(G::PageDown), "grid.page_down", Label::ActionGridPageDown, grid_focused),
    mv(Action::Grid(G::PageUp), "grid.page_up", Label::ActionGridPageUp, grid_focused),
    mv(Action::Grid(G::HalfDown), "grid.half_down", Label::ActionGridHalfDown, grid_focused),
    mv(Action::Grid(G::HalfUp), "grid.half_up", Label::ActionGridHalfUp, grid_focused),
    act(Action::Grid(G::Top), "grid.top", Label::ActionGridTop, grid_focused),
    act(Action::Grid(G::Bottom), "grid.bottom", Label::ActionGridBottom, grid_focused),
    act(Action::Grid(G::FirstCol), "grid.first_col", Label::ActionGridFirstCol, grid_focused),
    act(Action::Grid(G::LastCol), "grid.last_col", Label::ActionGridLastCol, grid_focused),
    act(Action::Grid(G::ViewCell), "grid.view_cell", Label::ActionGridViewCell, grid_focused),
    act(Action::Grid(G::CopyCell), "grid.copy_cell", Label::ActionGridCopyCell, grid_focused),
    act(Action::Grid(G::CopyRow), "grid.copy_row", Label::ActionGridCopyRow, grid_focused),
    act(Action::Grid(G::Select), "grid.select", Label::ActionGridSelect, grid_focused),
    act(Action::Grid(G::SelectRows), "grid.select_rows", Label::ActionGridSelectRows, grid_focused),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::TsvPlain),
        "results.copy.selection.tsv",
        Label::ActionResultsCopySelectionTsv,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::TsvHeader),
        "results.copy.selection.tsv_header",
        Label::ActionResultsCopySelectionTsvHeader,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::List),
        "results.copy.selection.list",
        Label::ActionResultsCopySelectionList,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::Csv),
        "results.copy.selection.csv",
        Label::ActionResultsCopySelectionCsv,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::Json),
        "results.copy.selection.json",
        Label::ActionResultsCopySelectionJson,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::JsonPretty),
        "results.copy.selection.json_pretty",
        Label::ActionResultsCopySelectionJsonPretty,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::Markdown),
        "results.copy.selection.markdown",
        Label::ActionResultsCopySelectionMarkdown,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::Html),
        "results.copy.selection.html",
        Label::ActionResultsCopySelectionHtml,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::Xml),
        "results.copy.selection.xml",
        Label::ActionResultsCopySelectionXml,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::SqlIn),
        "results.copy.selection.in",
        Label::ActionResultsCopySelectionIn,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::SqlInsert),
        "results.copy.selection.insert",
        Label::ActionResultsCopySelectionInsert,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Selection, CopyFormat::SqlUpdate),
        "results.copy.selection.update",
        Label::ActionResultsCopySelectionUpdate,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::TsvPlain),
        "results.copy.all.tsv",
        Label::ActionResultsCopyAllTsv,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::TsvHeader),
        "results.copy.all.tsv_header",
        Label::ActionResultsCopyAllTsvHeader,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::List),
        "results.copy.all.list",
        Label::ActionResultsCopyAllList,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::Csv),
        "results.copy.all.csv",
        Label::ActionResultsCopyAllCsv,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::Json),
        "results.copy.all.json",
        Label::ActionResultsCopyAllJson,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::JsonPretty),
        "results.copy.all.json_pretty",
        Label::ActionResultsCopyAllJsonPretty,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::Markdown),
        "results.copy.all.markdown",
        Label::ActionResultsCopyAllMarkdown,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::Html),
        "results.copy.all.html",
        Label::ActionResultsCopyAllHtml,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::Xml),
        "results.copy.all.xml",
        Label::ActionResultsCopyAllXml,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::SqlIn),
        "results.copy.all.in",
        Label::ActionResultsCopyAllIn,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::SqlInsert),
        "results.copy.all.insert",
        Label::ActionResultsCopyAllInsert,
        has_rows,
    ),
    act(
        Action::Copy(CopyScope::Fetched, CopyFormat::SqlUpdate),
        "results.copy.all.update",
        Label::ActionResultsCopyAllUpdate,
        has_rows,
    ),
    act(Action::ToggleDetail, "results.detail", Label::ActionResultsDetail, has_tab),
    act(Action::DetailTab, "results.detail_tab", Label::ActionResultsDetailTab, detail_panel),
    act(Action::Panel(PanelAction::Toggle), "results.panel.toggle", Label::ActionResultsPanelToggle, pane_known),
    act(Action::Panel(PanelAction::Maximize), "results.panel.maximize", Label::ActionResultsPanelMaximize, pane_shown),
    mv(Action::Panel(PanelAction::Grow), "results.panel.grow", Label::ActionResultsPanelGrow, pane_sized),
    mv(Action::Panel(PanelAction::Shrink), "results.panel.shrink", Label::ActionResultsPanelShrink, pane_sized),
    act(Action::ResultTab(true), "results.tab.next", Label::ActionResultsTabNext, result_tabs),
    act(Action::ResultTab(false), "results.tab.prev", Label::ActionResultsTabPrev, result_tabs),
    mv(Action::PageNext, "results.page.next", Label::ActionResultsPageNext, rows_shown),
    mv(Action::PagePrev, "results.page.prev", Label::ActionResultsPagePrev, rows_shown),
    act(Action::CountRows, "results.count", Label::ActionResultsCount, rows_shown),
];

pub fn spec(a: Action) -> &'static ActionSpec {
    REGISTRY.iter().find(|s| s.action == a).expect("every action is registered")
}

pub fn by_id(id: &str) -> Option<&'static ActionSpec> {
    REGISTRY.iter().find(|s| s.id == id)
}

/// Fuzzy subsequence score (case-insensitive, per `char`). Higher is better; `None` when
/// `query` is not a subsequence of `text`. Bonuses for word starts and consecutive runs.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    score(query, text, false)
}

/// [`fuzzy_score`], but the first character of `query` must start a word of `text`. The
/// command line uses it, so a short word that is not a command (`:w`) does not pick some
/// action that merely contains its letters.
pub fn word_score(query: &str, text: &str) -> Option<i32> {
    score(query, text, true)
}

fn score(query: &str, text: &str, anchored: bool) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let boundary = |i: usize| i == 0 || matches!(t[i - 1], ' ' | '.' | '_' | '-' | '/' | '(' | '→' | ':');
    let mut best: Option<i32> = None;
    // Try every start position of the first query char; greedy match after it.
    for start in (0..t.len()).filter(|&i| t[i] == q[0] && (!anchored || boundary(i))) {
        let mut score = 0;
        let mut qi = 0;
        let mut prev: Option<usize> = None;
        for (i, &c) in t.iter().enumerate().skip(start) {
            if qi < q.len() && c == q[qi] {
                score += 1;
                if boundary(i) {
                    score += 8;
                }
                match prev {
                    Some(p) if p + 1 == i => score += 5,
                    Some(p) => score -= ((i - p - 1) as i32).min(3),
                    None => score -= (start as i32).min(5),
                }
                prev = Some(i);
                qi += 1;
            }
        }
        if qi == q.len() {
            best = Some(best.map_or(score, |b: i32| b.max(score)));
        }
    }
    best
}

/// Registry indices matching `query` ([`word_score`]), best first (ties keep registry order).
/// Each action is matched against its label in the UI language, its English label and its id,
/// so English words also find actions while the UI is in Korean.
pub fn search(query: &str, i18n: &I18n) -> Vec<usize> {
    let mut hits: Vec<(i32, usize)> = REGISTRY
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            [s.label.text(i18n.lang), s.label.text(Lang::En), s.id]
                .iter()
                .filter_map(|t| word_score(query, t))
                .max()
                .map(|sc| (sc, i))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests;
