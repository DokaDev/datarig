//! The default key bindings. This
//! table is the single source for key handling, the command line's key column and
//! `docs/keybindings.md`.
//!
//! Reserved keys belong to a widget (the editor's vim or standard keys, a dialog's own keys) or
//! to an action of a later step. They are not actions: pressing one hands the keys to the
//! focused widget. Declaring them lets the conflict check keep app bindings off them.

use super::Ctx;
use super::Ctx::*;
use datarig_core::i18n::Label;

/// A condition a binding needs to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    /// The editor has a selection (standard mode: `Ctrl+C` copies instead of cancelling).
    Selection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindTarget {
    /// An action id of the registry (`app::action::REGISTRY`).
    Action(&'static str),
    /// Handled by a widget, or kept for a later action; the note says which.
    Reserved(&'static str),
    /// A prefix of longer bindings (a leader group) and its label. A group without any action
    /// under it is not shown.
    Group(Label),
}

#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub ctx: Ctx,
    pub keys: &'static str,
    pub target: BindTarget,
    pub when: Option<Cond>,
}

/// Keys no inner context may hide: they run, save, open the command line or help and
/// move between panes and tabs from any mode. `Space` is the leader of the non-text panes.
pub const PROTECTED: &[&str] = &[
    "ctrl+q",
    "ctrl+k",
    "ctrl+e",
    "ctrl+enter",
    "ctrl+s",
    "ctrl+t",
    "ctrl+o",
    "ctrl+g",
    "f1",
    "f6",
    "shift+f6",
    "ctrl+pagedown",
    "ctrl+pageup",
    "space",
];

/// The leader key: an unfinished `Space …` sequence is dropped, never handed to a widget.
pub const LEADER: &str = "space";

/// Leader groups. Groups whose actions arrive in a later step (`r`, `m`)
/// are declared already and stay hidden until they have an action.
const GROUPS: &[(Ctx, &str, Label)] = &[
    (Nav, "space", Label::GroupLeader),
    (Nav, "space c", Label::GroupConn),
    (Nav, "space t", Label::GroupTab),
    (Nav, "space s", Label::GroupScript),
    (Nav, "space r", Label::GroupResult),
    (Nav, "space r y", Label::GroupCopySelection),
    (Nav, "space r a", Label::GroupCopyAll),
    (Nav, "space m", Label::GroupMonitor),
];

/// The hint line: per context, the most relevant actions, best first, with their
/// short label. The keys shown are the ones bound (remapping included); narrow terminals drop
/// entries from the end.
pub const HINTS: &[(Ctx, &[(&str, Label)])] = &[
    (
        Explorer,
        &[
            ("explorer.activate", Label::HintOpen),
            ("pane.next", Label::HintPane),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
            ("app.quit", Label::HintQuit),
        ],
    ),
    (ExplorerFilter, &[("explorer.filter_accept", Label::HintDone), ("explorer.filter_clear", Label::HintClear)]),
    (
        Welcome,
        &[
            ("conn.new", Label::HintNew),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
            ("app.quit", Label::HintQuit),
        ],
    ),
    (
        Grid,
        &[
            ("grid.view_cell", Label::HintView),
            ("results.page.next", Label::HintNextPage),
            ("grid.copy_cell", Label::HintCopy),
            ("grid.select", Label::HintSelect),
            ("results.detail", Label::HintDetail),
            ("results.detail_tab", Label::HintDetailTab),
            ("pane.back", Label::HintBack),
            ("pane.next", Label::HintPane),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
        ],
    ),
    (
        VimNormal,
        &[
            ("query.execute_current", Label::HintRun),
            ("pane.next", Label::HintPane),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
        ],
    ),
    (
        VimVisual,
        &[
            ("query.execute_current", Label::HintRun),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
        ],
    ),
    (
        VimInsert,
        &[
            ("query.execute_current", Label::HintRun),
            ("editor.complete", Label::HintComplete),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
        ],
    ),
    (
        Standard,
        &[
            ("query.execute_current", Label::HintRun),
            ("editor.complete", Label::HintComplete),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
        ],
    ),
    (
        Inspector,
        &[
            ("results.detail_tab", Label::HintDetailTab),
            ("pane.back", Label::HintBack),
            ("results.detail", Label::HintDetail),
            ("commands.open", Label::HintCommands),
            ("help.context", Label::HintHelp),
        ],
    ),
    (CellViewer, &[("query.execute_current", Label::HintRun), ("commands.open", Label::HintCommands)]),
];

const ACTIONS: &[(Ctx, &str, &str)] = &[
    // root: also while a dialog is open
    (Root, "ctrl+q", "app.quit"),
    (Root, "ctrl+c", "query.cancel"),
    (Root, "ctrl+k", "commands.open"),
    // the busy notice of the launch-time migration: quitting abandons it; Ctrl+C there quits
    // too (`query.cancel` has nothing else to cancel)
    (Busy, "q", "app.quit"),
    // the profile form: the SSH key file picker
    (ProfileForm, "ctrl+o", "form.pick_key_file"),
    // workspace
    (Workspace, "ctrl+enter", "query.execute_current"),
    (Workspace, "ctrl+e", "query.execute_current"),
    (Workspace, "ctrl+o", "conn.quick_connect"),
    (Workspace, "ctrl+t", "tab.new_console"),
    (Workspace, "ctrl+w", "tab.close"),
    (Workspace, "ctrl+s", "script.save"),
    (Workspace, "ctrl+pagedown", "tab.next"),
    (Workspace, "ctrl+pageup", "tab.prev"),
    (Workspace, "f6", "pane.next"),
    (Workspace, "shift+f6", "pane.prev"),
    (Workspace, "shift+tab", "pane.prev"),
    (Workspace, "f1", "help.context"),
    // nav
    (Nav, "tab", "pane.next"),
    (Nav, ":", "commands.open"),
    (Nav, "g t", "tab.next"),
    (Nav, "g T", "tab.prev"),
    (Nav, "space t n", "tab.new_console"),
    (Nav, "space t c", "tab.close"),
    (Nav, "space t u", "tab.reopen_closed"),
    (Nav, "space 1", "tab.goto.1"),
    (Nav, "space 2", "tab.goto.2"),
    (Nav, "space 3", "tab.goto.3"),
    (Nav, "space 4", "tab.goto.4"),
    (Nav, "space 5", "tab.goto.5"),
    (Nav, "space 6", "tab.goto.6"),
    (Nav, "space 7", "tab.goto.7"),
    (Nav, "space 8", "tab.goto.8"),
    (Nav, "space 9", "tab.goto.9"),
    (Nav, "space c c", "conn.quick_connect"),
    (Nav, "space c n", "conn.new"),
    (Nav, "space c e", "conn.edit_current"),
    (Nav, "space c t", "conn.test_current"),
    (Nav, "space c x", "conn.disconnect_current"),
    (Nav, "space c r", "conn.reconnect_current"),
    (Nav, "space c s", "tab.set_connection"),
    (Nav, "space c d", "tab.set_context"),
    (Nav, "space s s", "script.save"),
    (Nav, "space s a", "script.save_as"),
    (Nav, "space s r", "script.rename"),
    (Nav, "space s o", "script.open"),
    (Nav, "space s d", "script.delete"),
    (Nav, "space ,", "settings.open"),
    (Nav, "space /", "commands.open"),
    (Nav, "space ?", "help.context"),
    // explorer
    (Explorer, "j", "explorer.down"),
    (Explorer, "down", "explorer.down"),
    (Explorer, "k", "explorer.up"),
    (Explorer, "up", "explorer.up"),
    (Explorer, "l", "explorer.expand"),
    (Explorer, "right", "explorer.expand"),
    (Explorer, "h", "explorer.collapse"),
    (Explorer, "left", "explorer.collapse"),
    (Explorer, "enter", "explorer.activate"),
    (Explorer, "g g", "explorer.top"),
    (Explorer, "G", "explorer.bottom"),
    (Explorer, "r", "explorer.refresh"),
    (Explorer, "/", "explorer.filter"),
    (Explorer, "n", "conn.new"),
    (Explorer, "e", "conn.edit"),
    (Explorer, "c", "conn.duplicate"),
    (Explorer, "d", "explorer.delete"),
    (Explorer, "t", "conn.test"),
    (Explorer, "x", "conn.disconnect"),
    (Explorer, "o", "conn.open_console"),
    (Explorer, "m", "explorer.move"),
    (Explorer, "N", "folder.new"),
    (Explorer, "R", "explorer.rename"),
    (Explorer, "O", "explorer.new_console_here"),
    (Explorer, "esc", "explorer.back"),
    (Explorer, "q", "app.quit"),
    (ExplorerFilter, "esc", "explorer.filter_clear"),
    (ExplorerFilter, "enter", "explorer.filter_accept"),
    (ExplorerFilter, "down", "explorer.down"),
    (ExplorerFilter, "up", "explorer.up"),
    // welcome panel (no profiles)
    (Welcome, "n", "conn.new"),
    (Welcome, "enter", "conn.new"),
    (Welcome, "q", "app.quit"),
    // result grid
    (Grid, "h", "grid.left"),
    (Grid, "left", "grid.left"),
    (Grid, "j", "grid.down"),
    (Grid, "down", "grid.down"),
    (Grid, "k", "grid.up"),
    (Grid, "up", "grid.up"),
    (Grid, "l", "grid.right"),
    (Grid, "right", "grid.right"),
    (Grid, "pagedown", "grid.page_down"),
    (Grid, "pageup", "grid.page_up"),
    (Grid, "ctrl+d", "grid.half_down"),
    (Grid, "ctrl+u", "grid.half_up"),
    (Grid, "g g", "grid.top"),
    (Grid, "home", "grid.top"),
    (Grid, "G", "grid.bottom"),
    (Grid, "end", "grid.bottom"),
    (Grid, "0", "grid.first_col"),
    (Grid, "$", "grid.last_col"),
    (Grid, "enter", "grid.view_cell"),
    (Grid, "y", "grid.copy_cell"),
    (Grid, "Y", "grid.copy_row"),
    (Grid, "v", "grid.select"),
    (Grid, "V", "grid.select_rows"),
    (Nav, "space r y t", "results.copy.selection.tsv"),
    (Nav, "space r y T", "results.copy.selection.tsv_header"),
    (Nav, "space r y l", "results.copy.selection.list"),
    (Nav, "space r y c", "results.copy.selection.csv"),
    (Nav, "space r y j", "results.copy.selection.json"),
    (Nav, "space r y J", "results.copy.selection.json_pretty"),
    (Nav, "space r y m", "results.copy.selection.markdown"),
    (Nav, "space r y h", "results.copy.selection.html"),
    (Nav, "space r y x", "results.copy.selection.xml"),
    (Nav, "space r y n", "results.copy.selection.in"),
    (Nav, "space r y i", "results.copy.selection.insert"),
    (Nav, "space r y u", "results.copy.selection.update"),
    (Nav, "space r a t", "results.copy.all.tsv"),
    (Nav, "space r a T", "results.copy.all.tsv_header"),
    (Nav, "space r a l", "results.copy.all.list"),
    (Nav, "space r a c", "results.copy.all.csv"),
    (Nav, "space r a j", "results.copy.all.json"),
    (Nav, "space r a J", "results.copy.all.json_pretty"),
    (Nav, "space r a m", "results.copy.all.markdown"),
    (Nav, "space r a h", "results.copy.all.html"),
    (Nav, "space r a x", "results.copy.all.xml"),
    (Nav, "space r a n", "results.copy.all.in"),
    (Nav, "space r a i", "results.copy.all.insert"),
    (Nav, "space r a u", "results.copy.all.update"),
    (Grid, "i", "results.detail"),
    (Grid, "I", "results.detail_tab"),
    (Nav, "space r i", "results.detail"),
    (Nav, "space r I", "results.detail_tab"),
    // the results pane of a query tab
    (Grid, "z", "results.panel.maximize"),
    (Grid, "+", "results.panel.grow"),
    (Grid, "-", "results.panel.shrink"),
    (Nav, "space r h", "results.panel.toggle"),
    (Nav, "space r z", "results.panel.maximize"),
    (Nav, "space r +", "results.panel.grow"),
    (Nav, "space r -", "results.panel.shrink"),
    (Grid, "n", "results.page.next"),
    (Grid, "p", "results.page.prev"),
    (Grid, "#", "results.count"),
    (Nav, "space r n", "results.page.next"),
    (Nav, "space r p", "results.page.prev"),
    (Nav, "space r #", "results.count"),
    (Grid, "L", "results.tab.next"),
    (Grid, "H", "results.tab.prev"),
    (Nav, "space r ]", "results.tab.next"),
    (Nav, "space r [", "results.tab.prev"),
    // the inspector, once clicked: Tab switches its tabs too
    (Inspector, "tab", "results.detail_tab"),
    (Inspector, "shift+tab", "results.detail_tab"),
    (Inspector, "I", "results.detail_tab"),
    (Inspector, "i", "results.detail"),
    (Inspector, "esc", "pane.back"),
    (Inspector, "q", "pane.back"),
    (Grid, "esc", "pane.back"),
    (Grid, "q", "pane.back"),
    // editor
    (VimInsert, "ctrl+n", "editor.complete"),
    (VimInsert, "f4", "editor.complete"),
    (Standard, "ctrl+n", "editor.complete"),
    (Standard, "f4", "editor.complete"),
];

/// Every vim command key, including those the editor does not implement yet.
const VIM: &[&str] = &[
    // motions
    "h", "j", "k", "l", "w", "W", "b", "B", "e", "E", "g e", "0", "^", "$", "g g", "G", "f", "F", "t", "T", ";", ",",
    "%", "{", "}", "(", ")", "[", "]", "H", "M", "L", "left", "right", "up", "down", "home", "end",
    // operators (text objects `iw`, `a(`, … follow them)
    "d", "c", "y", ">", "<", "=", "g ~", "g u", "g U", // counts, repeat, registers
    "1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "\"", // edits
    "x", "X", "s", "S", "r", "R", "p", "P", "u", "ctrl+r", "J", "~", // modes
    "i", "a", "I", "A", "o", "O", "v", "V", "ctrl+v", "esc", // search
    "/", "?", "n", "N", "*", "#", // scrolling
    "ctrl+d", "ctrl+u", "ctrl+f", "ctrl+b", "z z", "z t", "z b",
    // macros (a later step); also keeps `q` from quitting while editing
    "q",
];

const VIM_INSERT: &[&str] = &[
    "esc",
    "enter",
    "tab",
    "backspace",
    "delete",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "ctrl+w",
    "ctrl+u",
    "ctrl+p",
];

const STANDARD: &[&str] = &[
    "ctrl+x",
    "ctrl+v",
    "ctrl+z",
    "ctrl+y",
    "ctrl+a",
    "shift+left",
    "shift+right",
    "shift+up",
    "shift+down",
    "shift+home",
    "shift+end",
    "ctrl+left",
    "ctrl+right",
    "ctrl+backspace",
    "tab",
    "shift+tab",
    "esc",
    "enter",
    "backspace",
    "delete",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
];

/// Editing keys of a one-line text input.
const TEXT: &[&str] = &["backspace", "delete", "left", "right", "home", "end", "ctrl+u", "ctrl+a"];

const RESERVED: &[(Ctx, &[&str], &str)] = &[
    (Workspace, &["ctrl+g"], "editor.open_external"),
    (VimNormal, VIM, "vim"),
    (VimVisual, VIM, "vim"),
    (VimInsert, VIM_INSERT, "vim Insert"),
    (Standard, STANDARD, "standard editing"),
    (Commands, &["esc", "enter", "up", "down", "ctrl+p", "ctrl+n", "tab", "shift+tab"], "command line"),
    (Commands, TEXT, "text input"),
    (ExplorerFilter, TEXT, "text input"),
    (QuickConnect, &["esc", "enter", "up", "down", "ctrl+p", "ctrl+n"], "quick connect"),
    // `→`/`←` open and close the databases and schemas; the filter keeps its other
    // editing keys.
    (QuickConnect, &["right", "left"], "quick connect: databases and schemas"),
    (QuickConnect, &["backspace", "delete", "home", "end", "ctrl+u", "ctrl+a"], "text input"),
    (
        ProfileForm,
        &["tab", "shift+tab", "down", "up", "enter", "esc", "ctrl+s", "ctrl+t", "ctrl+n", "ctrl+p"],
        "profile form",
    ),
    (Chooser, &["j", "k", "down", "up", "enter", "/", "esc", "q"], "chooser"),
    (ChooserFilter, &["esc", "enter", "down", "up"], "chooser filter"),
    (ChooserFilter, TEXT, "text input"),
    (NameInput, &["enter", "esc"], "name input"),
    (
        ScriptTree,
        &["j", "k", "h", "l", "up", "down", "left", "right", "enter", "n", "tab", "shift+tab", "esc"],
        "saved-queries tree",
    ),
    (ScriptTreeName, &["up", "down", "enter", "tab", "shift+tab", "esc"], "saved-queries tree: name or filter"),
    (ScriptTreeName, TEXT, "text input"),
    (Settings, &["j", "k", "down", "up", "h", "l", "left", "right", "enter", "space", "esc", "q"], "settings screen"),
    (ContextMenu, &["j", "k", "down", "up", "enter", "esc", "q"], "context menu"),
    (NameInput, TEXT, "text input"),
    (ProfileForm, TEXT, "text input"),
    (Password, &["esc", "enter", "tab", "shift+tab"], "password prompt"),
    (Password, TEXT, "text input"),
    (Confirm, &["y", "n", "enter", "esc"], "confirmation (Enter keeps what would be lost)"),
    (
        RunConfirm,
        &["y", "n", "enter", "esc", "tab", "shift+tab", "left", "right", "h", "l"],
        "run confirmation (Cancel has the focus)",
    ),
    (
        IconsAsk,
        &["y", "n", "enter", "esc", "tab", "shift+tab", "left", "right", "h", "l"],
        "the icons question (No has the focus)",
    ),
    (CellViewer, &["j", "k", "down", "up", "pagedown", "pageup", "space", "g g", "G", "esc", "q"], "cell viewer"),
    (Completion, &["up", "down", "ctrl+p", "ctrl+n", "tab", "enter", "esc"], "completion popup"),
    (WhichKey, &["esc", "backspace"], "which-key popup"),
    (
        Help,
        &["j", "k", "down", "up", "pagedown", "pageup", "enter", "l", "right", "h", "left", "/", "esc", "q"],
        "keyboard help",
    ),
    (HelpFilter, &["esc", "enter", "down", "up"], "keyboard help filter"),
    (HelpFilter, TEXT, "text input"),
];

/// Every default binding, in table order.
pub fn defaults() -> Vec<Binding> {
    let mut v: Vec<Binding> = ACTIONS
        .iter()
        .map(|&(ctx, keys, id)| Binding { ctx, keys, target: BindTarget::Action(id), when: None })
        .collect();
    // Standard mode: Ctrl+C copies when there is a selection, otherwise it cancels the query.
    v.push(Binding {
        ctx: Standard,
        keys: "ctrl+c",
        target: BindTarget::Reserved("copy"),
        when: Some(Cond::Selection),
    });
    v.extend(GROUPS.iter().map(|&(ctx, keys, label)| Binding {
        ctx,
        keys,
        target: BindTarget::Group(label),
        when: None,
    }));
    for &(ctx, keys, note) in RESERVED {
        v.extend(keys.iter().map(|&k| Binding { ctx, keys: k, target: BindTarget::Reserved(note), when: None }));
    }
    v
}
