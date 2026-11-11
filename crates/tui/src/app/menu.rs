//! The action menu: one popup for the mouse and the keyboard. A right click opens it at the
//! pointer; `Space Space`, `Shift+F10` and the Menu key open it next to the selection (the
//! explorer's row, the grid's cell, the editor's cursor; `Space t m` the active tab's).
//!
//! It lists the actions for the thing it was opened on first (an explorer node, the grid's cell
//! or selection, a tab, the editor's statement or selection), then the pane's own, each with the
//! key that runs it in that pane. The explorer's come from its key bindings (the list the
//! keyboard help shows), so an action added to the explorer shows up here too; what has nothing
//! to do on the node is left out. The grid's copies open a second level, the formats; its edits
//! are listed while their keys would act on the cell.
//!
//! An action that would do something there but cannot now, for a reason that can be said (a
//! read-only connection, a table tab that has no text to save, paging that closed), is shown
//! dimmed with that reason in a few words where its keys would be. The keys and the wheel step
//! over it and the pointer does not select it; picking it (a click, its key) says the reason in
//! full and keeps the menu open. When a whole result cannot be edited, the reason is shown once,
//! on its first edit.
//!
//! Typing filters the items: names that start with the text first, then names with a word that
//! starts with it, then names that have its letters in order. When no item matches, every action
//! is searched as the `:` line does and listed under a heading of its own.
//!
//! What the menu was opened on is kept: when it has changed by the time an item is picked (the
//! tree was reloaded, the result was replaced, the tab went), nothing runs.

use super::action::{ChartAction, MonitorAction, PanelAction, PlanAction, PlanView};
use super::column_menu::ColumnItem;
use super::copy::{CopyFormat, CopyScope};
use super::edit::{EditAction, NotOffered};
use super::monitor::MonitorView;
use super::table_filter::TableAction;
use super::tabs::ResultView;
use super::*;
use crate::keymap::{Target, keys};
use crate::widgets::text_input::InputResult;
use datarig_core::chart::Kind as ChartKind;

/// Explorer actions that are not about a node (moving the cursor, quitting): never in the menu.
const NOT_IN_MENU: &[Action] = &[
    Action::Explorer(ExplorerAction::Down),
    Action::Explorer(ExplorerAction::Up),
    Action::Explorer(ExplorerAction::Top),
    Action::Explorer(ExplorerAction::Bottom),
    Action::Explorer(ExplorerAction::Expand),
    Action::Explorer(ExplorerAction::Collapse),
    Action::Explorer(ExplorerAction::FilterClear),
    Action::Explorer(ExplorerAction::FilterAccept),
    Action::Explorer(ExplorerAction::Back),
    Action::Explorer(ExplorerAction::ContextMenu),
    Action::Quit,
];

/// Explorer actions that do the same on every row: the pane's part of its menu (unless the row
/// gives one its own meaning, as "new" in the Tunnels section).
const EXPLORER_PANE: &[Action] =
    &[Action::NewProfile, Action::Explorer(ExplorerAction::NewFolder), Action::Explorer(ExplorerAction::Filter)];

/// The grid's actions for its cell or selection, in order; the selection's copy scope follows.
const GRID_MENU: &[Action] = &[
    Action::Grid(GridAction::ViewCell),
    Action::ToggleDetail,
    Action::Grid(GridAction::CopyCell),
    Action::Grid(GridAction::CopyRow),
];

/// The grid's filters by the value of its cell or selection (a table tab's), after its copies.
const FILTER_MENU: &[Action] = &[Action::Table(TableAction::ColumnMenu)];

/// The edits of the grid's cell or selection, after its copies: each only while its key would
/// act now ([`App::edit_offered`]).
const EDIT_MENU: &[EditAction] = &[
    EditAction::Cell,
    EditAction::SetNull,
    EditAction::SetDefault,
    EditAction::AddRow,
    EditAction::CloneRow,
    EditAction::DeleteRow,
    EditAction::PasteRows,
    EditAction::PasteRegister,
    EditAction::Undo,
    EditAction::Preview,
    EditAction::Apply,
    EditAction::Discard,
];

/// The results pane's actions, after the copies of every row.
const RESULTS_MENU: &[Action] = &[
    Action::Export,
    Action::Grid(GridAction::SelectAll),
    Action::Chart(ChartAction::Toggle),
    Action::ExplainAsPlan,
    Action::CountRows,
    Action::ResultTab(true),
    Action::ResultTab(false),
    Action::Panel(PanelAction::Maximize),
];

/// The editor's actions for the statement under the cursor or the selection.
const EDITOR_MENU: &[Action] =
    &[Action::RunStatement, Action::FormatSql, Action::ToggleComment, Action::Explain(false), Action::Explain(true)];

/// The Plan tab's actions for the selected node and the plan, then its pane's: the views.
const PLAN_MENU: &[Action] =
    &[Action::Plan(PlanAction::Detail), Action::Plan(PlanAction::CopyText), Action::Plan(PlanAction::CopyJson)];
const PLAN_PANE_MENU: &[Action] = &[
    Action::Plan(PlanAction::View(PlanView::Tree)),
    Action::Plan(PlanAction::View(PlanView::Summary)),
    Action::Plan(PlanAction::View(PlanView::Icicle)),
    Action::Plan(PlanAction::View(PlanView::Flame)),
    Action::Plan(PlanAction::View(PlanView::Timeline)),
    Action::Plan(PlanAction::View(PlanView::Rows)),
    Action::Plan(PlanAction::View(PlanView::Treemap)),
    Action::Plan(PlanAction::View(PlanView::Boxes)),
    Action::Plan(PlanAction::View(PlanView::Raw)),
    Action::ResultTab(true),
    Action::ResultTab(false),
    Action::Panel(PanelAction::Maximize),
];

/// The Chart tab's actions for the point at the cursor and the chart, then its pane's: the
/// kinds and the columns.
const CHART_MENU: &[Action] =
    &[Action::Chart(ChartAction::GotoRow), Action::Chart(ChartAction::CopyData), Action::Chart(ChartAction::CopyText)];
const CHART_PANE_MENU: &[Action] = &[
    Action::Chart(ChartAction::Kind(ChartKind::Bar)),
    Action::Chart(ChartAction::Kind(ChartKind::HBar)),
    Action::Chart(ChartAction::Kind(ChartKind::Line)),
    Action::Chart(ChartAction::PickX),
    Action::Chart(ChartAction::PickY),
    Action::Chart(ChartAction::PickBy),
    Action::Chart(ChartAction::Log),
    Action::Chart(ChartAction::Toggle),
    Action::ResultTab(true),
    Action::ResultTab(false),
    Action::Panel(PanelAction::Maximize),
];

/// The Monitor tab's actions for the selected session, then its pane's.
const MONITOR_MENU: &[Action] = &[
    Action::Monitor(MonitorAction::Details),
    Action::Monitor(MonitorAction::Cancel),
    Action::Monitor(MonitorAction::Terminate),
    Action::Monitor(MonitorAction::OpenConsole),
    Action::Monitor(MonitorAction::CopyQuery),
    Action::Monitor(MonitorAction::Format),
];
const MONITOR_PANE_MENU: &[Action] = &[
    Action::Monitor(MonitorAction::View(MonitorView::Overview)),
    Action::Monitor(MonitorAction::View(MonitorView::Sessions)),
    Action::Monitor(MonitorAction::View(MonitorView::Locks)),
    Action::Monitor(MonitorAction::View(MonitorView::Replication)),
    Action::Monitor(MonitorAction::ServerInfo),
    Action::Monitor(MonitorAction::Actions),
    Action::Monitor(MonitorAction::Pause),
    Action::Monitor(MonitorAction::ReadNow),
    Action::Monitor(MonitorAction::ShowIdle),
];

/// A DDL tab's actions for its text, and its pane's.
const DDL_MENU: &[Action] = &[Action::ReloadDdl, Action::DdlToConsole, Action::CopyDdl];
const DDL_PANE_MENU: &[Action] = &[Action::ExternalEdit];

/// The query pane's own actions.
const QUERY_MENU: &[Action] = &[
    Action::ExternalEdit,
    Action::ScriptSave,
    Action::ScriptSaveAs,
    Action::SetTabConnection,
    Action::SetTabContext,
    Action::TxCommit,
    Action::TxRollback,
    Action::TxSummary,
    Action::Panel(PanelAction::Toggle),
];

/// The actions of a tab.
const TAB_MENU: &[Action] = &[
    Action::CloseTab,
    Action::MoveTab(false),
    Action::MoveTab(true),
    Action::GrabTab,
    Action::ScriptSave,
    Action::ScriptSaveAs,
    Action::ScriptRename,
    Action::ScriptDelete,
    Action::SetTabConnection,
    Action::SetTabContext,
    Action::TxCommit,
    Action::TxRollback,
    Action::TxSummary,
    Action::ReconnectCurrent,
    Action::DisconnectCurrent,
];

/// The tab bar's own actions.
const TABS_MENU: &[Action] = &[Action::NewTab, Action::TabList, Action::ReopenTab, Action::ScriptOpen];

/// The layout's actions, after the editor's own (the tab bar's also resize the explorer; the
/// results' zoom is their maximise).
const LAYOUT_MENU: &[Action] = &[Action::TabList, Action::Zoom, Action::ToggleExplorer];
const EXPLORER_LAYOUT_MENU: &[Action] =
    &[Action::Zoom, Action::ToggleExplorer, Action::ExplorerWidth(false), Action::ExplorerWidth(true)];

/// The welcome panel's (no profile yet).
const WELCOME_MENU: &[Action] = &[Action::NewProfile, Action::OpenSettings, Action::Help];

/// Actions that open a menu: not offered by the search of a menu.
const OPENS_MENU: &[Action] = &[Action::OpenMenu, Action::TabMenu, Action::Explorer(ExplorerAction::ContextMenu)];

/// A menu's items: the target's, the pane's, and those of them shown dimmed with why.
type Items = (Vec<MenuItem>, Vec<MenuItem>, Vec<(MenuItem, Off)>);

/// The filter menu's items, those of them shown dimmed with why, their labels, the hints
/// shown with them, and the filter text each one that can run sends.
pub(super) type FilterItems = (
    Vec<MenuItem>,
    Vec<(MenuItem, Off)>,
    Vec<(MenuItem, Localized)>,
    Vec<(MenuItem, Localized)>,
    Vec<(MenuItem, String)>,
);

/// What a copy of the grid's menu takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuScope {
    Copy(CopyScope),
    /// Every row: the rest is fetched first (asked first), then every row is copied.
    FetchFirst,
    /// The format `y` and `Y` copy in (`copy_format`): a format is chosen, nothing copied.
    Default,
}

/// An item of a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Action(Action),
    /// Opens the formats of a copy scope (the second level).
    Scope(MenuScope),
    /// A format of a scope's submenu.
    Copy(MenuScope, CopyFormat),
    /// An entry of the column menu (on the grid's target, as the count choice's).
    Column(ColumnItem),
    /// An entry of the choice `#` opens on a filtered table tab (on the grid's target as the
    /// filter menu's are).
    Count(super::estimate::CountChoice),
}

/// What the menu was opened on, as it was then. An item runs only while it is still so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    /// An explorer row, and the names of what a schema tree's node stands for (a reloaded tree
    /// may put another node in its place).
    Explorer(explorer::Row, String),
    /// The active tab's results: the result shown, its selected cell (when it has rows) and
    /// selected range.
    Grid {
        tab: TabId,
        binding: u64,
        query: u64,
        shown: Option<usize>,
        view: ResultView,
        cell: Option<(usize, usize)>,
        range: Option<((usize, usize), crate::widgets::grid::Shape)>,
    },
    /// The active tab's plan: the run and statement it came from, its view and selected node.
    Plan { tab: TabId, binding: u64, query: u64, index: usize, view: PlanView, node: usize },
    /// The active tab's chart: the result it draws, what it draws and the point at its cursor.
    Chart { tab: TabId, binding: u64, result: u64, spec: datarig_core::chart::Spec, point: usize },
    /// The active tab's editor: its text, cursor and mode.
    Editor { tab: TabId, binding: u64, version: u64, cursor: (usize, usize), mode: Mode },
    /// The active tab.
    Tab { tab: TabId, binding: u64 },
    /// The active Monitor tab: its selected session (in the Sessions view) and the connection
    /// the list it is on was read on.
    Monitor { tab: TabId, key: Option<datarig_core::monitor::SessionKey>, read_on: u64 },
    /// The welcome panel (no profile yet).
    Welcome,
    /// The filters by value of the grid's cell or selection: the grid as it was (its target),
    /// and the result its cells were read from ([`crate::widgets::grid::ResultSet::id`]): rows
    /// read again in their place are another result.
    Filter { grid: Box<MenuTarget>, result: u64 },
}

/// A heading of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading {
    /// What the menu was opened on (named after it).
    Target,
    /// The pane's own actions.
    Pane,
    /// No item matches the filter: every action that does.
    AllActions,
}

/// Why an item cannot run now: what its key or a click on it says, and the few words a menu shows
/// next to it (`None`: a menu leaves it out, it has nothing to do there; its key still says why).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Off {
    pub short: Option<Label>,
    pub notice: Notice,
}

impl Off {
    /// Shown dimmed in a menu with `short`; picking it says `notice`.
    pub fn new(short: Label, notice: Notice) -> Self {
        Self { short: Some(short), notice }
    }

    /// Left out of a menu; its key says `notice`.
    pub fn said(notice: Notice) -> Self {
        Self { short: None, notice }
    }
}

/// A line of the menu as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuLine {
    Heading(Heading),
    Item(MenuItem),
    /// Nothing matches the filter, not even among every action.
    NoMatch,
    /// A rule between groups of the target's items (the column menu's), while nothing is typed.
    Rule,
}

/// The second level of a menu: the formats of a copy scope, next to its row.
pub struct SubMenu {
    pub scope: MenuScope,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    /// Screen area of the items, kept by the renderer (mouse).
    pub list: Rect,
}

pub struct ContextMenu {
    /// Whose menu it is: its keys are this context's.
    pub ctx: Ctx,
    pub target: MenuTarget,
    /// The items for the target and the pane's, as opened (the filter picks from these).
    pub own: Vec<MenuItem>,
    pub pane: Vec<MenuItem>,
    /// The items of `own` and `pane` that cannot run now, shown dimmed, and why.
    pub off: Vec<(MenuItem, Off)>,
    /// The own wording of an item, as it was when the menu opened: the explorer's of an action
    /// for the row (where the console opens, what is deleted), a filter's with its value.
    names: Vec<(MenuItem, Localized)>,
    /// A few words shown with an item that can run, where its keys would be (a filter's
    /// caveat).
    hints: Vec<(MenuItem, Localized)>,
    /// The filter text each filter item that can run sends, as written when the
    /// menu opened: what runs is what the menu showed, never written again from the cells.
    filters: Vec<(MenuItem, String)>,
    /// The explorer row's or the tab's name as shown when the menu opened (its heading).
    title: String,
    /// The focus to give back when the menu closes without running anything.
    back: Focus,
    /// What was typed to filter the items.
    pub filter: TextInput,
    /// Typing filters the items (a filter line on top); without, letters move and run the
    /// keys shown (a menu of a few entries).
    pub typing: bool,
    /// The lines shown: headings and the items that match the filter.
    pub lines: Vec<MenuLine>,
    /// The selected line (an item, when there is one).
    pub selected: usize,
    /// The first line shown (kept by the renderer so the selected one is on screen).
    pub scroll: usize,
    /// Where it was opened: its top-left corner is on the row below (clamped by the renderer).
    pub at: (u16, u16),
    /// Screen area of the lines, kept by the renderer (mouse).
    pub list: Rect,
    /// Screen area of the whole box, kept by the renderer (a click there keeps it open).
    pub area: Rect,
    /// The open submenu, which has the keys.
    pub sub: Option<SubMenu>,
}

impl ContextMenu {
    /// The actions of its items as opened that can run (not the scopes), the target's first.
    pub fn actions(&self) -> Vec<Action> {
        self.own
            .iter()
            .chain(&self.pane)
            .filter(|i| self.off_of(**i).is_none())
            .filter_map(|i| if let MenuItem::Action(a) = i { Some(*a) } else { None })
            .collect()
    }

    /// Why `item` cannot run now, when it is shown dimmed.
    pub fn off_of(&self, item: MenuItem) -> Option<&Off> {
        self.off.iter().find(|(i, _)| *i == item).map(|(_, o)| o)
    }

    /// Whether line `i` is an item that can run (not a heading, not a dimmed item).
    fn live(&self, i: usize) -> bool {
        self.item_at(i).is_some_and(|it| self.off_of(it).is_none())
    }

    /// The selected item, if the selected line is one that can run.
    pub fn selected_item(&self) -> Option<MenuItem> {
        self.item_at(self.selected).filter(|it| self.off_of(*it).is_none())
    }

    /// The item on line `i`, if it is one.
    pub fn item_at(&self, i: usize) -> Option<MenuItem> {
        match self.lines.get(i) {
            Some(MenuLine::Item(it)) => Some(*it),
            _ => None,
        }
    }

    /// Select the next (`d = 1`) or previous item, around the ends; headings and dimmed items
    /// are skipped.
    fn step(&mut self, d: isize) {
        let n = self.lines.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.selected as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if self.live(i as usize) {
                self.selected = i as usize;
                return;
            }
        }
    }
}

/// How well `query` matches the label shown, `texts[0]`, or one of the other `texts` (its English
/// label, its id): 0 when the label starts with it, 1 when a word of the label does, 2 when the
/// label has its letters in order (case ignored); 3 to 5 the same for another text, which ranks
/// below anything the label matches; `None` when nothing matches.
pub(crate) fn rank(query: &str, texts: &[&str]) -> Option<u8> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    texts
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let below = if i == 0 { 0 } else { 3 };
            let t = t.to_lowercase();
            let word_start = |i: usize| i == 0 || t[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric());
            let r = if t.starts_with(&q) {
                Some(0)
            } else if t.match_indices(&q).any(|(i, _)| word_start(i)) {
                Some(1)
            } else {
                action::fuzzy_score(&q, &t).map(|_| 2)
            };
            r.map(|r| r + below)
        })
        .min()
}

impl App {
    /// Whether `a` does something on the explorer row `row`.
    fn applies(&self, a: Action, row: &explorer::RowKind) -> bool {
        use explorer::RowKind as R;
        let profile = match row {
            R::Profile(id)
            | R::ProfileError(id)
            | R::Database(id, _)
            | R::DatabasesNote(id)
            | R::DatabaseNote(id, _)
            | R::Node(id, _)
            | R::AuxNode(id, _, _) => Some(*id),
            _ => None,
        };
        let connected = profile.is_some_and(|id| self.conns.is_connected(id));
        let script = matches!(row, R::Script(_) | R::ScriptFolder(_));
        let in_database = matches!(row, R::Node(..) | R::AuxNode(..) | R::Database(..) | R::DatabaseNote(..));
        let preset = matches!(row, R::Tunnel(_) | R::TunnelError(_));
        // A profile listed under a preset: `Enter` goes to it, nothing else.
        if matches!(row, R::TunnelUser(..)) {
            return a == Action::Explorer(ExplorerAction::Activate);
        }
        // A profile listed under an environment or the default rules: the same.
        if matches!(row, R::EnvironmentUser(..) | R::DefaultRulesUser(_)) {
            return a == Action::Explorer(ExplorerAction::Activate);
        }
        // The "Environments" section: a new environment, and an environment's edit, copy,
        // rename and delete; the presets' row adds them.
        let env = matches!(row, R::Environment(_));
        // The default rules: edited only.
        if *row == R::DefaultRules {
            return match a {
                Action::NewProfile | Action::NewEnvironment | Action::EditProfile => true,
                Action::Explorer(ExplorerAction::Activate) => {
                    self.explorer_arrow(&explorer::Row::new(row.clone(), 1)).is_some()
                }
                _ => false,
            };
        }
        if env || matches!(row, R::EnvironmentsHeader | R::EnvironmentPresets) {
            return match a {
                Action::NewProfile | Action::NewEnvironment => true,
                Action::Explorer(ExplorerAction::Activate) => {
                    !env || self.explorer_arrow(&explorer::Row::new(row.clone(), 1)).is_some()
                }
                Action::EditProfile
                | Action::DuplicateProfile
                | Action::Explorer(ExplorerAction::Rename)
                | Action::Explorer(ExplorerAction::Delete) => env,
                _ => false,
            };
        }
        // A profile listed under a session preset: `Enter` goes to it, nothing else.
        if matches!(row, R::SessionUser(..)) {
            return a == Action::Explorer(ExplorerAction::Activate);
        }
        // The "Session presets" section: a new preset, and a preset's edit, copy, test and
        // delete.
        let session = matches!(row, R::Session(_) | R::SessionError(_));
        if session || matches!(row, R::SessionsHeader | R::SessionsEmpty) {
            return match a {
                Action::NewProfile | Action::NewSession => true,
                Action::Explorer(ExplorerAction::Activate) => match row {
                    R::SessionsEmpty | R::SessionError(_) => false,
                    R::Session(_) => self.explorer_arrow(&explorer::Row::new(row.clone(), 1)).is_some(),
                    _ => true,
                },
                Action::EditProfile
                | Action::DuplicateProfile
                | Action::TestConnection
                | Action::Explorer(ExplorerAction::Delete) => session,
                _ => false,
            };
        }
        let tunnels = preset || matches!(row, R::TunnelsHeader | R::TunnelsEmpty);
        // The "Tunnels" section: a new preset, and a preset's edit, copy, delete and test.
        if tunnels {
            return match a {
                Action::NewProfile | Action::NewTunnel => true,
                Action::Explorer(ExplorerAction::Activate) => !matches!(row, R::TunnelsEmpty),
                Action::EditProfile
                | Action::DuplicateProfile
                | Action::TestConnection
                | Action::Explorer(ExplorerAction::Delete) => preset,
                _ => false,
            };
        }
        match a {
            Action::Explorer(ExplorerAction::Activate) => !matches!(row, R::NewConnection | R::ScriptsEmpty),
            Action::Explorer(ExplorerAction::Refresh) => connected || script || matches!(row, R::ScriptsHeader),
            Action::Explorer(ExplorerAction::Delete) => matches!(row, R::Profile(_) | R::Folder(_)) || script,
            Action::Explorer(ExplorerAction::Rename) => matches!(row, R::Folder(_)) || script,
            Action::Explorer(ExplorerAction::Move) => profile.is_some() || script,
            Action::Explorer(ExplorerAction::ConsoleHere) => in_database,
            // On a relation or what is under it (an index, a trigger: its own); a trigger's
            // function on a trigger.
            Action::Explorer(ExplorerAction::ShowDdl) => self.ddl_menu(row, false),
            Action::Explorer(ExplorerAction::ShowFunctionDdl) => self.ddl_menu(row, true),
            // In a database or schema the console opens there (`O`), not with the profile's
            // defaults: one console item, which says where.
            Action::OpenConsole => profile.is_some() && !in_database,
            Action::EditProfile | Action::DuplicateProfile | Action::TestConnection => profile.is_some(),
            Action::Disconnect => profile.is_some_and(|id| self.conns.state(id) != NodeState::Disconnected),
            // A connection whose driver monitors its server.
            Action::Monitor(super::monitor::MonitorAction::Open) => profile.is_some_and(|id| self.monitor_on(id)),
            // Profile and folder actions of the explorer have nothing to do on the scripts.
            Action::NewProfile | Action::Explorer(ExplorerAction::NewFolder) => {
                !script && !matches!(row, R::ScriptsHeader | R::ScriptsEmpty)
            }
            _ => true,
        }
    }

    /// Whether "Show DDL" (`function`: the trigger function's) has an object on `row`, with a
    /// driver that reads DDL.
    fn ddl_menu(&self, row: &explorer::RowKind, function: bool) -> bool {
        self.ddl_object_of_row(row, function).is_some_and(|(id, _, _)| self.ddl_on(id))
    }

    /// The explorer's actions for `row`, in its binding order: the row's own, and the pane's.
    fn explorer_sections(&self, row: &explorer::Row) -> (Vec<Action>, Vec<Action>) {
        let (mut own, mut pane): (Vec<Action>, Vec<Action>) = (Vec::new(), Vec::new());
        for e in self.keymap.section(Ctx::Explorer, Some(Ctx::Explorer)) {
            let a = e.action;
            if own.contains(&a)
                || pane.contains(&a)
                || NOT_IN_MENU.contains(&a)
                || !(action::spec(a).when)(self)
                || !self.applies(a, &row.kind)
            {
                continue;
            }
            if EXPLORER_PANE.contains(&a) && self.explorer_menu_label(a, row).is_none() {
                pane.push(a);
            } else {
                own.push(a);
            }
        }
        (own, pane)
    }

    /// The menu's actions for the explorer row under the cursor: the row's, then the pane's.
    pub fn menu_items(&self) -> Vec<Action> {
        let Some(row) = self.explorer_row() else { return Vec::new() };
        let (mut own, pane) = self.explorer_sections(&row);
        own.extend(pane);
        own
    }

    /// The label of explorer action `a` in the menu, for the row `row` it runs on: what it
    /// deletes, moves or renames there, and where the console opens. `None`: the action's own
    /// label.
    fn explorer_menu_label(&self, a: Action, row: &explorer::Row) -> Option<Localized> {
        use explorer::RowKind as R;
        let label = match (a, &row.kind) {
            (Action::Explorer(ExplorerAction::ConsoleHere), _) => {
                let (id, ctx) = self.console_here_context(row)?;
                let place = match (ctx.database, ctx.schema) {
                    (Some(db), Some(schema)) => format!("{db}.{schema}"),
                    (None, Some(schema)) => schema,
                    (Some(db), None) => db,
                    (None, None) => self.own_database(id),
                };
                return Some(self.i18n.msg(&Msg::MenuConsoleIn { place }));
            }
            (Action::Explorer(ExplorerAction::ShowDdl), _) => Label::MenuShowDdl,
            (Action::Explorer(ExplorerAction::ShowFunctionDdl), _) => Label::MenuShowFunctionDdl,
            (Action::Explorer(ExplorerAction::Rename), R::Script(_)) => Label::MenuRenameQuery,
            (Action::Explorer(ExplorerAction::Rename), R::Folder(_) | R::ScriptFolder(_)) => Label::MenuRenameFolder,
            (Action::Explorer(ExplorerAction::Delete), R::Script(_)) => Label::MenuDeleteQuery,
            (Action::Explorer(ExplorerAction::Delete), R::Folder(_) | R::ScriptFolder(_)) => Label::MenuDeleteFolder,
            (Action::Explorer(ExplorerAction::Delete), R::Profile(_)) => Label::MenuDeleteProfile,
            (Action::Explorer(ExplorerAction::Move), R::Script(_)) => Label::MenuMoveQuery,
            (Action::Explorer(ExplorerAction::Move), R::ScriptFolder(_)) => Label::MenuMoveFolder,
            (Action::Explorer(ExplorerAction::Move), _) => Label::MenuMoveProfile,
            (
                Action::NewProfile,
                R::TunnelsHeader | R::TunnelsEmpty | R::Tunnel(_) | R::TunnelError(_) | R::TunnelUser(..),
            ) => Label::MenuNewTunnel,
            (Action::EditProfile, R::Tunnel(_) | R::TunnelError(_) | R::TunnelUser(..)) => Label::MenuEditTunnel,
            (Action::DuplicateProfile, R::Tunnel(_) | R::TunnelError(_) | R::TunnelUser(..)) => {
                Label::MenuDuplicateTunnel
            }
            (Action::TestConnection, R::Tunnel(_) | R::TunnelError(_) | R::TunnelUser(..)) => Label::MenuTestTunnel,
            (Action::Explorer(ExplorerAction::Delete), R::Tunnel(_) | R::TunnelError(_) | R::TunnelUser(..)) => {
                Label::MenuDeleteTunnel
            }
            (Action::EditProfile, R::DefaultRules) => Label::MenuEditDefaultRules,
            (
                Action::NewProfile,
                R::EnvironmentsHeader | R::EnvironmentPresets | R::Environment(_) | R::DefaultRules,
            ) => Label::MenuNewEnvironment,
            (Action::EditProfile, R::Environment(_)) => Label::MenuEditEnvironment,
            (Action::DuplicateProfile, R::Environment(_)) => Label::MenuDuplicateEnvironment,
            (Action::Explorer(ExplorerAction::Rename), R::Environment(_)) => Label::MenuRenameEnvironment,
            (Action::Explorer(ExplorerAction::Delete), R::Environment(_)) => Label::MenuDeleteEnvironment,
            (Action::Explorer(ExplorerAction::Activate), R::EnvironmentPresets) => Label::MenuAddEnvironmentPresets,
            (Action::NewProfile, R::SessionsHeader | R::SessionsEmpty | R::Session(_) | R::SessionError(_)) => {
                Label::MenuNewSession
            }
            (Action::EditProfile, R::Session(_) | R::SessionError(_)) => Label::MenuEditSession,
            (Action::DuplicateProfile, R::Session(_) | R::SessionError(_)) => Label::MenuDuplicateSession,
            (Action::TestConnection, R::Session(_) | R::SessionError(_)) => Label::MenuTestSession,
            (Action::Explorer(ExplorerAction::Delete), R::Session(_) | R::SessionError(_)) => Label::MenuDeleteSession,
            _ => return None,
        };
        Some(self.i18n.label(label))
    }

    /// The target of the kind of `like` as it is now, if there is one.
    fn menu_target_now(&self, like: &MenuTarget) -> Option<MenuTarget> {
        match like {
            MenuTarget::Explorer(..) => {
                let row = self.explorer_row().filter(|_| self.focus == Focus::Tree)?;
                let key = crate::widgets::explorer::row_key(self, &row);
                Some(MenuTarget::Explorer(row, key))
            }
            MenuTarget::Grid { .. } => self.grid_target(),
            MenuTarget::Plan { .. } => self.plan_target(),
            MenuTarget::Chart { .. } => self.chart_target(),
            MenuTarget::Editor { .. } => self.editor_target(),
            MenuTarget::Tab { .. } => self.tab_target(),
            MenuTarget::Monitor { .. } => self.monitor_target(),
            MenuTarget::Welcome => self.profiles.is_empty().then_some(MenuTarget::Welcome),
            MenuTarget::Filter { grid, .. } => {
                let grid = Box::new(self.menu_target_now(grid)?);
                let result = self.grid_result_id()?;
                Some(MenuTarget::Filter { grid, result })
            }
        }
    }

    fn grid_target(&self) -> Option<MenuTarget> {
        if self.profiles.is_empty() || self.tabs.is_empty() || !matches!(self.focus, Focus::Results | Focus::Inspector)
        {
            return None;
        }
        let t = self.tab();
        // The Messages of a run hide the grid: no cell to act on then. A new row (shown after the
        // rows read) is a cell too, for its edits only.
        // A hidden column is no cell either (every column hidden).
        let rows = t.exec.view == ResultView::Rows
            && t.grid.shown(t.grid.col)
            && matches!(&t.results, Results::Rows(rs) if t.grid.row < rs.rows.len() + rs.edit.new_shown());
        Some(MenuTarget::Grid {
            tab: t.id,
            binding: t.binding,
            query: t.exec.query_id,
            shown: t.exec.shown,
            view: t.exec.view,
            cell: rows.then_some((t.grid.row, t.grid.col)),
            range: t.grid.anchor.filter(|_| rows).map(|a| (a, t.grid.shape)),
        })
    }

    fn plan_target(&self) -> Option<MenuTarget> {
        if !self.plan_shown() || !matches!(self.focus, Focus::Results | Focus::Inspector) {
            return None;
        }
        let t = self.tab();
        let p = t.exec.plan.as_ref()?;
        Some(MenuTarget::Plan {
            tab: t.id,
            binding: t.binding,
            query: p.query,
            index: p.index,
            view: p.view,
            node: p.selected,
        })
    }

    /// The Plan tab's menu: the selected node's and the plan's actions, then the views.
    fn open_plan_menu(&mut self, at: (u16, u16)) {
        let back = self.focus;
        self.focus = Focus::Results;
        let Some(target) = self.plan_target() else {
            self.focus = back;
            return;
        };
        let mut off = Vec::new();
        let (own, pane) = (self.available(PLAN_MENU, &mut off), self.available(PLAN_PANE_MENU, &mut off));
        self.push_menu_from(back, Ctx::Plan, target, (own, pane, off), at);
        if !self.overlays.is_open(OverlayKind::ContextMenu) {
            self.focus = back;
        }
    }

    /// The Plan tab's menu next to its selected node (the pane's corner when it is not drawn).
    fn open_plan_menu_here(&mut self) {
        let r = self.layout.results;
        let at = self
            .tab()
            .exec
            .plan
            .as_ref()
            .and_then(|p| p.hits.iter().find(|(_, i)| *i == p.selected).map(|(rect, _)| (rect.x + 2, rect.y)))
            .unwrap_or((r.x + 2, r.y + 1));
        self.open_plan_menu(at);
    }

    fn chart_target(&self) -> Option<MenuTarget> {
        if !self.chart_shown() || !self.chart_live() || !matches!(self.focus, Focus::Results | Focus::Inspector) {
            return None;
        }
        let t = self.tab();
        let c = t.exec.chart.as_ref()?;
        // The result shown now (the chart follows it only when it is drawn).
        let Results::Rows(rs) = &t.results else { return None };
        Some(MenuTarget::Chart { tab: t.id, binding: t.binding, result: rs.id, spec: c.spec.clone(), point: c.cursor })
    }

    /// The Chart tab's menu: the point's and the chart's actions, then the kinds and columns.
    fn open_chart_menu(&mut self, at: (u16, u16)) {
        let back = self.focus;
        self.focus = Focus::Results;
        self.chart_sync();
        let Some(target) = self.chart_target() else {
            self.focus = back;
            return;
        };
        let mut off = Vec::new();
        let (own, pane) = (self.available(CHART_MENU, &mut off), self.available(CHART_PANE_MENU, &mut off));
        self.push_menu_from(back, Ctx::Chart, target, (own, pane, off), at);
        if !self.overlays.is_open(OverlayKind::ContextMenu) {
            self.focus = back;
        }
    }

    /// The Chart tab's menu next to the cursor's point (the plot's corner when it is not drawn).
    fn open_chart_menu_here(&mut self) {
        let r = self.layout.results;
        let at = self
            .tab()
            .exec
            .chart
            .as_ref()
            .and_then(|c| c.hits.iter().find(|(_, i)| *i == c.cursor).map(|(rect, _)| (rect.x + 1, rect.y)))
            .unwrap_or((r.x + 2, r.y + 1));
        self.open_chart_menu(at);
    }

    fn editor_target(&self) -> Option<MenuTarget> {
        if self.profiles.is_empty() || self.tabs.is_empty() || self.focus != Focus::Editor || self.tab().is_table() {
            return None;
        }
        let t = self.tab();
        let e = &t.editor;
        Some(MenuTarget::Editor {
            tab: t.id,
            binding: t.binding,
            version: e.version(),
            cursor: (e.row, e.col),
            mode: e.mode,
        })
    }

    fn tab_target(&self) -> Option<MenuTarget> {
        if self.profiles.is_empty() || self.tabs.is_empty() {
            return None;
        }
        Some(MenuTarget::Tab { tab: self.tab().id, binding: self.tab().binding })
    }

    fn monitor_target(&self) -> Option<MenuTarget> {
        if !self.monitor_shown() {
            return None;
        }
        let tab = self.tab().id;
        let m = self.monitor_of(tab)?;
        let key = match m.view == MonitorView::Sessions && m.server_info.is_none() {
            true => m.selected_row().map(|r| r.key),
            false => None,
        };
        Some(MenuTarget::Monitor { tab, key, read_on: m.read_on })
    }

    /// Open a menu of `own` and `pane` items for `target` at `at` (nothing when both are empty),
    /// `off` of them dimmed with their reasons.
    fn push_menu(&mut self, ctx: Ctx, target: MenuTarget, items: Items, at: (u16, u16)) {
        self.push_menu_from(self.focus, ctx, target, items, at);
    }

    /// [`App::push_menu`], giving the focus back to `back` when it closes without running
    /// anything.
    fn push_menu_from(&mut self, back: Focus, ctx: Ctx, target: MenuTarget, (own, pane, off): Items, at: (u16, u16)) {
        if own.is_empty() && pane.is_empty() {
            return;
        }
        // However it opened (a right click too): the first step about the menu is taken.
        self.step_taken(Action::OpenMenu);
        if !self.tabs.is_empty() {
            let t = self.tab_mut();
            t.popup = None;
            t.completion_due = None;
        }
        let title = match &target {
            MenuTarget::Explorer(row, _) => crate::widgets::explorer::row_name(self, row),
            MenuTarget::Tab { tab, .. } | MenuTarget::Monitor { tab, .. } => {
                self.tabs.get(*tab).map(|t| crate::widgets::tabbar::document_name(self, t)).unwrap_or_default()
            }
            _ => String::new(),
        };
        let names = match &target {
            MenuTarget::Explorer(row, _) => own
                .iter()
                .chain(&pane)
                .filter_map(|i| if let MenuItem::Action(a) = i { Some(*a) } else { None })
                .filter_map(|a| self.explorer_menu_label(a, row).map(|l| (MenuItem::Action(a), l)))
                .collect(),
            _ => Vec::new(),
        };
        self.overlays.push(Overlay::ContextMenu(ContextMenu {
            ctx,
            target,
            own,
            pane,
            off,
            names,
            hints: Vec::new(),
            filters: Vec::new(),
            title,
            back,
            filter: TextInput::default(),
            typing: true,
            lines: Vec::new(),
            selected: 0,
            scroll: 0,
            at,
            list: Rect::default(),
            area: Rect::default(),
            sub: None,
        }));
        self.menu_refilter();
    }

    /// The actions of `list` that can run now, as items, and those that cannot for a reason a
    /// menu shows (added to `off` with it).
    fn available(&self, list: &[Action], off: &mut Vec<(MenuItem, Off)>) -> Vec<MenuItem> {
        let mut items = Vec::new();
        for &a in list {
            let item = MenuItem::Action(a);
            if (action::spec(a).when)(self) {
                items.push(item);
            } else if let Some(o) = self.unavailable_why(a).filter(|o| o.short.is_some()) {
                items.push(item);
                off.push((item, o));
            }
        }
        items
    }

    /// Right click on the explorer at (x, y): select the row there (blank space: the
    /// "＋ New connection" row) and open its menu.
    pub(super) fn open_context_menu(&mut self, x: u16, y: u16) {
        self.focus = Focus::Tree;
        self.explorer.filtering = false;
        let rows = self.explorer_rows();
        let area = self.explorer.area;
        let i = if y <= area.y { 0 } else { 1 + self.explorer.scroll + usize::from(y - area.y - 1) };
        self.explorer.select(&rows, if i < rows.len() { i } else { 0 });
        self.open_explorer_menu((x, y));
    }

    /// The explorer's menu for the row under its cursor, at `at`.
    fn open_explorer_menu(&mut self, at: (u16, u16)) {
        self.focus = Focus::Tree;
        self.explorer.filtering = false;
        let Some(row) = self.explorer_row() else { return };
        let (own, pane) = self.explorer_sections(&row);
        let key = crate::widgets::explorer::row_key(self, &row);
        let items = |v: Vec<Action>| v.into_iter().map(MenuItem::Action).collect();
        self.push_menu(Ctx::Explorer, MenuTarget::Explorer(row, key), (items(own), items(pane), Vec::new()), at);
    }

    /// `explorer.context_menu`, or the action menu in the explorer: the menu next to the
    /// cursor's row.
    pub(super) fn open_context_menu_here(&mut self) {
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        let area = self.explorer.area;
        let y = if i == 0 { area.y } else { area.y + 1 + (i - 1).saturating_sub(self.explorer.scroll) as u16 };
        self.open_explorer_menu((area.x + 4, y.min(area.y + area.height.saturating_sub(1))));
    }

    /// Right click on the grid at (x, y): on the selected range, the menu for that range;
    /// elsewhere the cell there is selected (the range dropped) and the menu is for it.
    pub(super) fn open_grid_menu(&mut self, x: u16, y: u16) {
        self.focus = Focus::Results;
        if self.plan_shown() {
            self.plan_click(x, y, false);
            return self.open_plan_menu((x, y));
        }
        if self.chart_shown() {
            // The point there; the kinds and columns open their own choices on a left click only.
            self.chart_click(x, y, false);
            return self.open_chart_menu((x, y));
        }
        let table = self.tab().is_table();
        let t = self.tabs.active_mut();
        // A table tab's header: that column's menu (the cursor goes to its column).
        if table
            && t.exec.view == ResultView::Rows
            && let Some(c) = t.grid.header_col(x, y)
        {
            t.grid.col = c;
            t.grid.anchor = None;
            t.grid.detached = false;
            return self.open_column_menu_at((x, y));
        }
        if let (ResultView::Rows, Results::Rows(rs)) = (t.exec.view, &t.results) {
            let (total, ncols) = (rs.rows.len(), rs.columns.len());
            let on_range = t.grid.cell_at(x, y, total).is_some_and(|c| t.grid.in_range(c, total, ncols));
            if !on_range && t.grid.click(x, y, rs.rows.len()) {
                t.grid.anchor = None;
            }
        }
        self.open_results_menu((x, y));
    }

    /// The results' menu: the selected cell's or range's actions, then the pane's.
    fn open_results_menu(&mut self, at: (u16, u16)) {
        // The grid's actions are the grid's: from the inspector the focus goes there, and back
        // when the menu closes without running anything.
        let back = self.focus;
        self.focus = Focus::Results;
        let Some(target) = self.grid_target() else { return };
        let cell = match &target {
            MenuTarget::Grid { cell, .. } => *cell,
            _ => None,
        };
        // A new row is no row of the result: nothing reads it but its edits.
        let read = cell.is_some_and(|(row, _)| row < self.shown_rows_read());
        let mut off = Vec::new();
        let mut own = if read { self.available(GRID_MENU, &mut off) } else { Vec::new() };
        let mut pane = Vec::new();
        if read {
            own.push(MenuItem::Scope(MenuScope::Copy(CopyScope::Selection)));
            own.extend(self.available(FILTER_MENU, &mut off));
        }
        if cell.is_some() {
            // Why the whole result takes no edit is said once, on the first edit; why one does
            // not act on this cell or selection, on that edit.
            let mut said = false;
            for &e in EDIT_MENU {
                let item = MenuItem::Action(Action::Edit(e));
                if !(action::spec(Action::Edit(e)).when)(self) {
                    continue;
                }
                match self.edit_offer(e) {
                    Ok(()) => own.push(item),
                    Err(NotOffered::Here(o)) => {
                        own.push(item);
                        off.push((item, *o));
                    }
                    Err(NotOffered::Result(o)) if !said => {
                        said = true;
                        own.push(item);
                        off.push((item, *o));
                    }
                    Err(_) => {}
                }
            }
        }
        if read {
            pane.push(MenuItem::Scope(MenuScope::Copy(CopyScope::Fetched)));
            let item = MenuItem::Scope(MenuScope::FetchFirst);
            if self.can_fetch_rest() {
                pane.push(item);
            } else if let Some(o) = self.fetch_off() {
                pane.push(item);
                off.push((item, o));
            }
            pane.push(MenuItem::Scope(MenuScope::Default));
        }
        pane.extend(self.available(RESULTS_MENU, &mut off));
        // No cell shown (every column hidden): none to select.
        if self.grid_no_cell() {
            pane.retain(|i| *i != MenuItem::Action(Action::Grid(GridAction::SelectAll)));
        }
        self.push_menu_from(back, Ctx::Grid, target, (own, pane, off), at);
        if !self.overlays.is_open(OverlayKind::ContextMenu) {
            self.focus = back;
        }
    }

    /// How many rows the active tab's shown result read (the rows after them are new rows).
    fn shown_rows_read(&self) -> usize {
        match &self.tab().results {
            Results::Rows(rs) => rs.rows.len(),
            _ => 0,
        }
    }

    /// The results' menu next to the grid's selected cell (the grid's corner without one).
    fn open_results_menu_here(&mut self) {
        let at = self.menu_at_cell();
        self.open_results_menu(at);
    }

    /// Where a menu about the grid's selected cell opens: next to it (the grid's corner when it
    /// is not shown; the inspector's corner while it is zoomed and the grid is not drawn).
    pub(super) fn menu_at_cell(&self) -> (u16, u16) {
        let r = self.layout.results;
        if r.width == 0 {
            let d = self.layout.detail;
            return (d.x + 2, d.y + 1);
        }
        let g = &self.tab().grid;
        let shown = g.row >= g.top && g.row < g.top + g.page_rows.max(1);
        let y = if shown { g.data_y + (g.row - g.top) as u16 } else { g.data_y };
        let x = g.hit_cols.iter().find(|c| c.2 == g.col).map_or(r.x + 2, |c| c.0 + 1);
        let clamp = |v: u16, lo: u16, len: u16| v.clamp(lo, lo + len.saturating_sub(1));
        (clamp(x, r.x, r.width), clamp(y, r.y, r.height))
    }

    /// The grid's cell and selection as a menu's target, as they are now.
    pub(super) fn menu_grid_target(&self) -> Option<MenuTarget> {
        self.grid_target()
    }

    /// The id of the result the active tab's grid shows, when it shows rows.
    pub(super) fn grid_result_id(&self) -> Option<u64> {
        if self.tabs.is_empty() {
            return None;
        }
        match &self.tab().results {
            Results::Rows(rs) => Some(rs.id),
            _ => None,
        }
    }

    /// Open the filter menu: `items` for `target` (the grid as it is) at `at`, headed `title`,
    /// `off` of them dimmed with why, `names` their labels and `hints` the words shown with
    /// them. The focus goes back to the grid when it closes without running anything.
    pub(super) fn push_filter_menu(
        &mut self,
        target: MenuTarget,
        title: String,
        (items, off, names, hints, filters): FilterItems,
        at: (u16, u16),
    ) {
        if items.is_empty() {
            return;
        }
        let back = self.focus;
        self.push_menu_from(back, Ctx::Grid, target, (items, Vec::new(), off), at);
        if let Some(m) = self.overlays.menu_mut()
            && matches!(m.target, MenuTarget::Filter { .. })
        {
            m.title = title;
            m.names = names;
            m.hints = hints;
            m.filters = filters;
        }
        self.menu_refilter();
    }

    /// Right click in the editor at (x, y): outside the Visual selection the cursor goes
    /// there first (as a click), so the menu is about the statement there.
    pub(super) fn open_editor_menu_at(&mut self, x: u16, y: u16) {
        self.focus = Focus::Editor;
        let i = self.layout.editor_text;
        if x >= i.x && x < i.x + i.width && y >= i.y && y < i.y + i.height {
            let (cx, cy) = (x - i.x, y - i.y);
            let e = &mut self.tab_mut().editor;
            if !e.in_selection(e.pos_at(cx, i32::from(cy))) {
                e.click(cx, cy);
            }
        }
        self.open_editor_menu((x, y));
    }

    /// The editor's menu: the statement's (or the selection's) actions, then the pane's.
    fn open_editor_menu(&mut self, at: (u16, u16)) {
        let Some(target) = self.editor_target() else { return };
        let ddl = self.tab().is_ddl();
        let ctx = match self.tab().editor.mode {
            Mode::Normal if ddl => Ctx::Ddl,
            Mode::Normal => Ctx::VimNormal,
            Mode::Visual => Ctx::VimVisual,
            Mode::Insert => Ctx::VimInsert,
        };
        let mut off = Vec::new();
        let (own, mut pane) = match ddl {
            true => (self.available(DDL_MENU, &mut off), self.available(DDL_PANE_MENU, &mut off)),
            false => (self.available(EDITOR_MENU, &mut off), self.available(QUERY_MENU, &mut off)),
        };
        pane.extend(self.available(LAYOUT_MENU, &mut off));
        self.push_menu(ctx, target, (own, pane, off), at);
    }

    /// Right click on the tab bar at (x, y): on a tab, that tab becomes active (as a click)
    /// and its menu opens.
    pub(super) fn tab_bar_menu(&mut self, x: u16, y: u16) {
        use crate::widgets::tabbar::TabHit;
        let Some(hit) = self.pointer.tab_hits.iter().find(|(a, b, _)| x >= *a && x < *b).map(|h| h.2) else { return };
        let index = match hit {
            TabHit::Tab(i) | TabHit::Close(i) => i,
            TabHit::More(_) => return,
        };
        self.overlays.close(OverlayKind::CellViewer);
        self.switch_tab(|m| m.activate(index));
        if self.tabs.active_index() == index {
            self.open_tab_menu((x, y));
        }
    }

    /// The active tab's menu: its actions, then the tab bar's.
    fn open_tab_menu(&mut self, at: (u16, u16)) {
        let Some(target) = self.tab_target() else { return };
        let mut off = Vec::new();
        let mut own = self.available(TAB_MENU, &mut off);
        // Disconnecting a tab's profile that is not connected does nothing.
        let gone = self.tab().profile.is_none_or(|id| self.conns.state(id) == NodeState::Disconnected);
        own.retain(|i| !(gone && *i == MenuItem::Action(Action::DisconnectCurrent)));
        let mut pane = self.available(TABS_MENU, &mut off);
        pane.extend(self.available(EXPLORER_LAYOUT_MENU, &mut off));
        self.push_menu(Ctx::Nav, target, (own, pane, off), at);
    }

    /// `tab.menu`: the active tab's menu under it in the tab bar.
    pub(super) fn open_tab_menu_here(&mut self) {
        use crate::widgets::tabbar::TabHit;
        let bar = self.layout.tab_bar;
        let active = self.tabs.active_index();
        let x = self.pointer.tab_hits.iter().find(|h| h.2 == TabHit::Tab(active)).map_or(bar.x, |h| h.0);
        self.open_tab_menu((x, bar.y));
    }

    /// The Monitor tab's menu next to its selected session: the session's actions (a cancel or
    /// an end the session refuses dimmed, with why), then the tab's.
    fn open_monitor_menu(&mut self) {
        use datarig_core::monitor::ActionKind;
        let Some(target) = self.monitor_target() else { return };
        let Some(m) = self.monitor_of(self.tab().id) else { return };
        let selected = m.view == MonitorView::Sessions && m.server_info.is_none() && m.selected_row().is_some();
        let rows = m.hits.rows;
        let line = m.list.cursor.saturating_sub(m.list.top) as u16;
        let r = self.layout.results;
        let at = match rows.height > 0 && line < rows.height {
            true => (rows.x + 2, rows.y + line),
            false => (r.x + 2, r.y + 1),
        };
        let mut off = Vec::new();
        let own = if selected { self.available(MONITOR_MENU, &mut off) } else { Vec::new() };
        for (a, kind) in
            [(MonitorAction::Cancel, ActionKind::Cancel), (MonitorAction::Terminate, ActionKind::Terminate)]
        {
            let item = MenuItem::Action(Action::Monitor(a));
            if own.contains(&item)
                && let Some(o) = self.monitor_action_off(kind)
            {
                off.push((item, o));
            }
        }
        let pane = self.available(MONITOR_PANE_MENU, &mut off);
        self.push_menu(Ctx::Monitor, target, (own, pane, off), at);
    }

    /// The welcome panel's menu (no profile yet).
    pub(super) fn open_welcome_menu(&mut self, at: (u16, u16)) {
        let mut off = Vec::new();
        let pane = self.available(WELCOME_MENU, &mut off);
        self.push_menu(Ctx::Welcome, MenuTarget::Welcome, (Vec::new(), pane, off), at);
    }

    /// `menu.open` (`Space Space`, `Shift+F10`, the Menu key): the focused pane's menu next to
    /// its selection.
    pub(super) fn open_action_menu(&mut self) {
        if self.profiles.is_empty() && self.focus != Focus::Tree {
            let e = self.layout.editor;
            return self.open_welcome_menu((e.x + 2, e.y + 1));
        }
        match self.focus {
            Focus::Tree => self.open_context_menu_here(),
            // No tab: only the explorer is there.
            _ if self.tabs.is_empty() => self.open_context_menu_here(),
            Focus::Results | Focus::Inspector if self.monitor_shown() => self.open_monitor_menu(),
            Focus::Results | Focus::Inspector if self.plan_shown() => self.open_plan_menu_here(),
            Focus::Results | Focus::Inspector if self.chart_shown() => self.open_chart_menu_here(),
            Focus::Results | Focus::Inspector => self.open_results_menu_here(),
            Focus::Editor => {
                self.close_search_prompt();
                let at = self.layout.editor_cursor;
                self.open_editor_menu(at);
            }
        }
    }

    /// The texts an item is found by: its label as shown, its English label and its id.
    fn item_texts(&self, ctx: Ctx, names: &[(MenuItem, Localized)], item: MenuItem) -> Vec<String> {
        let mut out = vec![self.menu_row(ctx, names, item).0.to_string()];
        match item {
            MenuItem::Action(a) => {
                let s = action::spec(a);
                out.push(s.label.text(datarig_i18n::Lang::En).to_string());
                out.push(s.id.to_string());
            }
            MenuItem::Scope(MenuScope::Copy(sc)) => out.push(sc.name().to_string()),
            _ => {}
        }
        out
    }

    /// Recompute the lines after the filter changed: the items that match it, best first in
    /// each section, the best one that can run selected; none: every action that matches, as
    /// `:` finds them.
    pub(super) fn menu_refilter(&mut self) {
        let Some(m) = self.overlays.menu() else { return };
        let query = m.filter.text().trim().to_string();
        let (ctx, names) = (m.ctx, m.names.clone());
        let off: Vec<MenuItem> = m.off.iter().map(|(i, _)| *i).collect();
        let sections = [(Heading::Target, m.own.clone()), (Heading::Pane, m.pane.clone())];
        let mut lines: Vec<MenuLine> = Vec::new();
        let mut best: Option<(u8, usize)> = None;
        for (heading, items) in sections {
            let mut ranked: Vec<(u8, usize, MenuItem)> = items
                .into_iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    let texts = self.item_texts(ctx, &names, item);
                    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
                    rank(&query, &texts).map(|r| (r, i, item))
                })
                .collect();
            if ranked.is_empty() {
                continue;
            }
            ranked.sort_by_key(|&(r, i, _)| (r, i));
            lines.push(MenuLine::Heading(heading));
            let mut group = None;
            for (r, _, item) in ranked {
                let g = match item {
                    MenuItem::Column(c) if query.is_empty() => Some(c.group()),
                    _ => None,
                };
                if group.is_some() && g.is_some() && g != group {
                    lines.push(MenuLine::Rule);
                }
                group = g;
                if !off.contains(&item) && best.is_none_or(|(b, _)| r < b) {
                    best = Some((r, lines.len()));
                }
                lines.push(MenuLine::Item(item));
            }
        }
        if lines.is_empty() {
            let found: Vec<MenuLine> = action::search(&query, &self.i18n)
                .into_iter()
                .map(|i| action::REGISTRY[i].action)
                .filter(|a| !OPENS_MENU.contains(a) && (action::spec(*a).when)(self))
                .map(|a| MenuLine::Item(MenuItem::Action(a)))
                .collect();
            if found.is_empty() {
                lines.push(MenuLine::NoMatch);
            } else {
                lines.push(MenuLine::Heading(Heading::AllActions));
                best = Some((0, 1));
                lines.extend(found);
            }
        }
        let Some(m) = self.overlays.menu_mut() else { return };
        m.lines = lines;
        m.selected = best.map_or(0, |(_, i)| i);
        m.scroll = 0;
        m.sub = None;
    }

    /// Open the formats of the scope on line `i` of the menu.
    fn open_sub(&mut self, i: usize) {
        let Some(m) = self.overlays.menu_mut() else { return };
        let Some(MenuItem::Scope(scope)) = m.item_at(i) else { return };
        m.selected = i;
        let (formats, selected) = match scope {
            // The choices of `copy_format`, the one it names selected.
            MenuScope::Default => {
                let now = CopyFormat::of_setting(self.prefs.copy_format);
                (&CopyFormat::DEFAULTS[..], CopyFormat::DEFAULTS.iter().position(|f| *f == now).unwrap_or(0))
            }
            _ => (&CopyFormat::MENU[..], 0),
        };
        let items = formats.iter().map(|f| MenuItem::Copy(scope, *f)).collect();
        m.sub = Some(SubMenu { scope, items, selected, list: Rect::default() });
    }

    /// `Space r y d`: the results' menu with its choice of the format `y` and `Y` copy in open.
    pub(super) fn open_copy_default(&mut self) {
        self.open_results_menu_here();
        let item = MenuLine::Item(MenuItem::Scope(MenuScope::Default));
        if let Some(i) = self.overlays.menu().and_then(|m| m.lines.iter().position(|x| *x == item)) {
            self.open_sub(i);
        }
    }

    /// Close the menu without running anything: the focus goes back where it was.
    fn menu_dismiss(&mut self) {
        if let Some(m) = self.overlays.menu() {
            self.focus = m.back;
        }
        self.overlays.close(OverlayKind::ContextMenu);
    }

    /// Whether what the menu was opened on is still what it was.
    fn menu_current(&self) -> bool {
        self.overlays.menu().is_some_and(|m| self.menu_target_now(&m.target).as_ref() == Some(&m.target))
    }

    /// Run `item` of the menu (it closes first), or open a scope's formats. Nothing runs when
    /// what the menu was opened on has changed since: that is said, and the menu closes.
    fn menu_run(&mut self, item: MenuItem) {
        // Choosing the format of `y` and `Y` copies nothing: it holds whatever the grid shows.
        let any_target = matches!(item, MenuItem::Scope(_) | MenuItem::Copy(MenuScope::Default, _));
        if !any_target && !self.menu_current() {
            self.menu_dismiss();
            return self.flash(Notice::new(Label::MenuStale, Level::Warning));
        }
        match item {
            MenuItem::Scope(_) => {
                let i = self.overlays.menu().and_then(|m| m.lines.iter().position(|x| *x == MenuLine::Item(item)));
                if let Some(i) = i {
                    self.open_sub(i);
                }
            }
            MenuItem::Action(a) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.dispatch_from(a, tips::Source::Menu);
            }
            MenuItem::Copy(MenuScope::Copy(scope), f) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.dispatch_from(Action::Copy(scope, f), tips::Source::Menu);
            }
            MenuItem::Copy(MenuScope::FetchFirst, f) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.ask_fetch_then_copy(f);
            }
            MenuItem::Copy(MenuScope::Default, f) => {
                self.menu_dismiss();
                if let Some(s) = f.setting() {
                    self.apply_setting(super::command::Setting::CopyFormat(s));
                }
            }
            MenuItem::Column(c) => {
                let text =
                    self.overlays.menu().and_then(|m| m.filters.iter().find(|(i, _)| *i == item)).map(|f| f.1.clone());
                self.overlays.close(OverlayKind::ContextMenu);
                self.column_item(c, text);
            }
            MenuItem::Count(choice) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.count_choice(choice);
            }
        }
    }

    /// Keys of the menu (`overlay.context_menu`, text input): typing filters it; `↓`/`↑`,
    /// `Ctrl+N`/`Ctrl+P`, `Tab`/`Shift+Tab` move; `Enter` runs (or opens a scope's formats, as
    /// `→` at the end of the filter does); `Esc` closes (`Backspace` on an empty filter does
    /// not). A key shown next to an item that is not a character (`Ctrl+…`, `F…`) runs it.
    /// In an open submenu the arrows move, `Enter`/`→` run, `Esc`/`←` go back to the first
    /// level and a format's letter (`t`, `c`, `j`, …) runs that format.
    pub(super) fn menu_key(&mut self, k: KeyChord, repeat: bool) {
        let Some(m) = self.overlays.menu_mut() else { return };
        let ctrl = k.mods == KeyModifiers::CONTROL;
        let plain = k.mods.is_empty() || k.mods == KeyModifiers::SHIFT;
        if let Some(sub) = m.sub.as_mut() {
            let n = sub.items.len().max(1);
            // Letters are the formats' keys here (`j` is JSON): the arrows move.
            match k.code {
                KeyCode::Down | KeyCode::Tab if plain => sub.selected = (sub.selected + 1) % n,
                KeyCode::Char('n') if ctrl => sub.selected = (sub.selected + 1) % n,
                KeyCode::Up | KeyCode::BackTab if plain => sub.selected = (sub.selected + n - 1) % n,
                KeyCode::Char('p') if ctrl => sub.selected = (sub.selected + n - 1) % n,
                _ if repeat => {}
                KeyCode::Esc | KeyCode::Left if k.mods.is_empty() => m.sub = None,
                KeyCode::Enter | KeyCode::Right if k.mods.is_empty() => {
                    let item = sub.items[sub.selected.min(sub.items.len() - 1)];
                    self.menu_run(item);
                }
                // A format's key (as after `Space r y`); Hangul typed with a Korean input source
                // is the QWERTY key at its place, as outside the menu.
                KeyCode::Char(c) if plain => {
                    let key = match crate::input::hangul::keys(c).as_deref() {
                        Some([k]) => *k,
                        _ => c,
                    };
                    if let Some(item) =
                        sub.items.iter().copied().find(|i| matches!(i, MenuItem::Copy(_, f) if f.key() == key))
                    {
                        self.menu_run(item);
                    }
                }
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Tab if k.mods.is_empty() => m.step(1),
            KeyCode::Char('n') if ctrl => m.step(1),
            KeyCode::Up if k.mods.is_empty() => m.step(-1),
            KeyCode::BackTab => m.step(-1),
            KeyCode::Char('p') if ctrl => m.step(-1),
            // With a modifier (`Ctrl+Enter`) they are the keys of items, below.
            KeyCode::Esc | KeyCode::Enter if repeat && k.mods.is_empty() => {}
            KeyCode::Esc if k.mods.is_empty() => self.menu_dismiss(),
            KeyCode::Enter if k.mods.is_empty() => {
                if let Some(item) = m.selected_item() {
                    self.menu_run(item);
                }
            }
            KeyCode::Right
                if !repeat
                    && matches!(m.selected_item(), Some(MenuItem::Scope(_)))
                    && m.filter.cursor() == m.filter.text().chars().count() =>
            {
                let i = m.selected;
                self.open_sub(i);
            }
            KeyCode::Char('j') if !m.typing && k.mods.is_empty() => m.step(1),
            KeyCode::Char('k') if !m.typing && k.mods.is_empty() => m.step(-1),
            // No filter: a key shown next to an entry runs it.
            _ if !m.typing => {
                if !repeat && let Some(item) = self.menu_item_of_key(k) {
                    self.menu_pick(item);
                }
            }
            _ => match m.filter.handle_key(&k.to_event()) {
                InputResult::Changed => self.menu_refilter(),
                InputResult::Moved => {}
                // Not text: the key shown next to an item of the menu (filtered out or not) or of the
                // actions listed runs it.
                _ if repeat => {}
                _ => {
                    let ctx = m.ctx;
                    let item = |a: Action| {
                        let it = MenuItem::Action(a);
                        m.own.contains(&it) || m.pane.contains(&it) || m.lines.contains(&MenuLine::Item(it))
                    };
                    if let (Some(Target::Action(a)), _) = self.keymap.resolve_seq(ctx, &[k])
                        && item(a)
                    {
                        self.menu_pick(MenuItem::Action(a));
                    }
                }
            },
        }
    }

    /// Text pasted while the menu has the keyboard: into its filter.
    pub(super) fn menu_paste(&mut self, text: &str) {
        let Some(m) = self.overlays.menu_mut() else { return };
        if m.sub.is_none() && m.typing {
            m.filter.insert_str(text);
            self.menu_refilter();
        }
    }

    /// The menu's line at screen row `y`, if `y` is on its lines.
    fn menu_line_at(m: &ContextMenu, at: ratatui::layout::Position) -> Option<usize> {
        m.list.contains(at).then(|| m.scroll + usize::from(at.y - m.list.y)).filter(|i| *i < m.lines.len())
    }

    /// `item` picked by a click or its key: run it, or (dimmed) say why it cannot run, the menu
    /// left open.
    fn menu_pick(&mut self, item: MenuItem) {
        match self.overlays.menu().and_then(|m| m.off_of(item)).map(|o| o.notice.clone()) {
            Some(why) => self.flash(why),
            None => self.menu_run(item),
        }
    }

    /// Mouse while the menu is open: a click on an item runs it (on a scope: opens its
    /// formats; on a dimmed item: says why it cannot run), elsewhere in the box (a heading, the
    /// filter, the border) does nothing, outside it closes it; the wheel moves the selection.
    pub(super) fn menu_mouse(&mut self, ev: MouseEvent) {
        let Some(m) = self.overlays.menu_mut() else { return };
        let at = ratatui::layout::Position::new(ev.column, ev.row);
        match ev.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = ev.kind == MouseEventKind::ScrollDown;
                match m.sub.as_mut() {
                    Some(sub) => {
                        let n = sub.items.len().max(1);
                        sub.selected = if down { (sub.selected + 1) % n } else { (sub.selected + n - 1) % n };
                    }
                    None => m.step(if down { 1 } else { -1 }),
                }
            }
            MouseEventKind::Down(_) => {
                if let Some(sub) = m.sub.as_ref()
                    && sub.list.contains(at)
                {
                    let item = sub.items.get(usize::from(ev.row - sub.list.y)).copied();
                    if let Some(item) = item {
                        self.menu_run(item);
                    }
                } else if m.list.contains(at) {
                    let item = Self::menu_line_at(m, at).and_then(|i| m.item_at(i));
                    if let Some(item) = item {
                        m.sub = None;
                        self.menu_pick(item);
                    }
                } else if !m.area.contains(at) {
                    self.menu_dismiss();
                }
            }
            _ => {}
        }
    }

    /// The pointer moved over the open menu (no button): the item under it is selected, as the
    /// keys would, and the keys go on from there. Over another item of the menu than the one
    /// whose formats are open, the formats close. `false` when nothing changed (off the items,
    /// on a heading, a dimmed item or the selected one): no frame is drawn for it.
    pub(super) fn menu_hover(&mut self, ev: MouseEvent) -> bool {
        let Some(m) = self.overlays.menu_mut() else { return false };
        let at = ratatui::layout::Position::new(ev.column, ev.row);
        if let Some(sub) = m.sub.as_mut()
            && sub.list.contains(at)
        {
            let i = usize::from(ev.row - sub.list.y);
            if i >= sub.items.len() || i == sub.selected {
                return false;
            }
            sub.selected = i;
            return true;
        }
        let Some(i) = Self::menu_line_at(m, at) else { return false };
        if !m.live(i) || i == m.selected {
            return false;
        }
        m.selected = i;
        m.sub = None;
        true
    }

    /// The label and keys of a menu item (keys in the menu's context).
    fn menu_row(&self, ctx: Ctx, names: &[(MenuItem, Localized)], item: MenuItem) -> (Localized, String) {
        let keys =
            |a: Action| self.keymap.hint_keys(a, ctx, self.enhanced_keys).map(|k| keys::label(&k)).unwrap_or_default();
        let own = names.iter().find(|(x, _)| *x == item).map(|(_, l)| l.clone());
        match item {
            MenuItem::Action(a) => (own.unwrap_or_else(|| self.i18n.label(action::spec(a).label)), keys(a)),
            MenuItem::Column(c) => (own.unwrap_or_else(|| Localized::verbatim("")), self.column_item_keys(c)),
            MenuItem::Count(_) => (own.unwrap_or_else(|| Localized::verbatim("")), String::new()),
            MenuItem::Scope(scope) => {
                let label = match scope {
                    MenuScope::Copy(CopyScope::Selection) => self.i18n.label(Label::CopyScopeSelection),
                    MenuScope::Copy(CopyScope::Fetched) => match &self.tab().results {
                        Results::Rows(rs) if rs.more || !matches!(self.tab().exec.paging, Paging::None) => {
                            self.i18n.msg(&Msg::CopyScopeFetchedPartial { count: rs.rows.len() as u64 })
                        }
                        _ => self.i18n.label(Label::CopyScopeFetched),
                    },
                    MenuScope::FetchFirst => self.i18n.label(Label::CopyScopeFetchFirst),
                    MenuScope::Default => self.i18n.label(Label::CopyScopeDefault),
                };
                // The mark of an item that opens more items.
                (Localized::verbatim(format!("{label} ▸")), String::new())
            }
            // In the submenu a format's own key runs it (in both scopes): that key is shown.
            // The choices of `copy_format` are named as the setting names them.
            MenuItem::Copy(MenuScope::Default, f) => {
                let label = self.i18n.label(f.default_label());
                let now = f == CopyFormat::of_setting(self.prefs.copy_format);
                (if now { self.copy_default_marked(label) } else { label }, f.key().to_string())
            }
            MenuItem::Copy(_, f) => {
                let label = self.i18n.label(f.menu_label());
                let default = f == self.copy_default_item();
                (if default { self.copy_default_marked(label) } else { label }, f.key().to_string())
            }
        }
    }

    /// The text of a heading of the open menu.
    fn menu_heading(&self, m: &ContextMenu, h: Heading) -> Localized {
        match (h, &m.target) {
            (Heading::AllActions, _) => self.i18n.label(Label::MenuHeadingAllActions),
            (
                Heading::Target,
                MenuTarget::Explorer(..)
                | MenuTarget::Tab { .. }
                | MenuTarget::Monitor { .. }
                | MenuTarget::Filter { .. },
            ) => Localized::verbatim(m.title.clone()),
            (Heading::Target, MenuTarget::Grid { range: Some(_), .. }) => self.i18n.label(Label::MenuHeadingSelection),
            (Heading::Target, MenuTarget::Grid { cell: Some((row, col)), .. }) => {
                let column = match &self.tab().results {
                    Results::Rows(rs) => rs.columns.get(*col).map(|c| c.meta.name.clone()).unwrap_or_default(),
                    _ => String::new(),
                };
                if *row >= self.shown_rows_read() {
                    return self.i18n.msg(&Msg::MenuHeadingNewCell { column });
                }
                self.i18n.msg(&Msg::MenuHeadingCell { column, row: (row + 1).to_string() })
            }
            (Heading::Target, MenuTarget::Editor { mode: Mode::Visual, .. }) => {
                self.i18n.label(Label::MenuHeadingSelection)
            }
            (Heading::Target, MenuTarget::Editor { .. }) => self.i18n.label(Label::MenuHeadingStatement),
            // The selected node, as the tree names it.
            (Heading::Target, MenuTarget::Plan { node, .. }) => {
                let name = self.tab().exec.plan.as_ref().map(|p| p.plan.label(*node)).unwrap_or_default();
                Localized::verbatim(name)
            }
            // The point at the cursor, as the chart labels it.
            (Heading::Target, MenuTarget::Chart { point, .. }) => {
                let c = self.tab().exec.chart.as_ref();
                let p = c.and_then(|c| c.model()).and_then(|m| m.points.get(*point).cloned());
                match p {
                    Some(p) if p.others => self.i18n.label(Label::ChartOthers),
                    Some(p) => Localized::verbatim(crate::text::sanitize_cell(&p.label)),
                    None => self.i18n.label(Label::ResultsTabChart),
                }
            }
            (Heading::Target, _) => self.i18n.label(Label::AppTitle),
            (Heading::Pane, MenuTarget::Plan { .. }) => self.i18n.label(Label::ResultsTabPlan),
            (Heading::Pane, MenuTarget::Chart { .. }) => self.i18n.label(Label::ResultsTabChart),
            (Heading::Pane, MenuTarget::Explorer(..)) => self.i18n.label(Label::PaneTreeTitle),
            (Heading::Pane, MenuTarget::Grid { .. } | MenuTarget::Filter { .. }) => {
                self.i18n.label(Label::PaneResultsTitle)
            }
            (Heading::Pane, MenuTarget::Editor { .. }) => self.i18n.label(Label::PaneEditorTitle),
            (Heading::Pane, MenuTarget::Tab { .. }) => self.i18n.label(Label::GroupTab),
            (Heading::Pane, MenuTarget::Monitor { .. }) => self.i18n.label(Label::GroupMonitor),
            (Heading::Pane, MenuTarget::Welcome) => self.i18n.label(Label::AppTitle),
        }
    }

    /// An item of menu `m` as drawn: its label, and its keys or (dimmed) why it cannot run.
    fn menu_entry(&self, m: &ContextMenu, item: MenuItem) -> (Localized, String) {
        let (label, keys) = self.menu_row(m.ctx, &m.names, item);
        match m.off_of(item).and_then(|o| o.short) {
            Some(why) => (label, self.i18n.label(why).to_string()),
            None => match m.hints.iter().find(|(i, _)| *i == item) {
                Some((_, hint)) => (label, hint.to_string()),
                None => (label, keys),
            },
        }
    }

    /// The open menu's lines as drawn: (heading?, label, keys or why a dimmed item cannot run).
    pub fn menu_lines(&self) -> Vec<(bool, Localized, String)> {
        let Some(m) = self.overlays.menu() else { return Vec::new() };
        m.lines
            .iter()
            .map(|l| match l {
                MenuLine::Heading(h) => (true, self.menu_heading(m, *h), String::new()),
                MenuLine::Item(i) => {
                    let (label, keys) = self.menu_entry(m, *i);
                    (false, label, keys)
                }
                MenuLine::NoMatch => (true, self.i18n.label(Label::MenuNoMatch), String::new()),
                // A heading with no text: drawn as a rule.
                MenuLine::Rule => (true, Localized::verbatim(""), String::new()),
            })
            .collect()
    }

    /// Which of the open menu's lines are dimmed items (they cannot run now).
    pub fn menu_dimmed(&self) -> Vec<bool> {
        let Some(m) = self.overlays.menu() else { return Vec::new() };
        m.lines.iter().map(|l| matches!(l, MenuLine::Item(i) if m.off_of(*i).is_some())).collect()
    }

    /// Which of the open menu's lines are items that can run shown with a hint where their keys
    /// would be.
    pub fn menu_hinted(&self) -> Vec<bool> {
        let Some(m) = self.overlays.menu() else { return Vec::new() };
        m.lines
            .iter()
            .map(|l| matches!(l, MenuLine::Item(i) if m.off_of(*i).is_none() && m.hints.iter().any(|(h, _)| h == i)))
            .collect()
    }

    /// The widths of the open menu's item labels (as opened, unfiltered), its headings and its
    /// keys, so the box keeps its width while the filter narrows it.
    pub fn menu_widths(&self) -> (usize, usize, usize) {
        let Some(m) = self.overlays.menu() else { return (0, 0, 0) };
        let rows: Vec<(Localized, String)> = m.own.iter().chain(&m.pane).map(|i| self.menu_entry(m, *i)).collect();
        let label = rows.iter().map(|(l, _)| crate::text::width(l)).max().unwrap_or(0);
        let keys = rows.iter().map(|(_, k)| crate::text::width(k)).max().unwrap_or(0);
        let headings = [Heading::Target, Heading::Pane]
            .iter()
            .map(|h| crate::text::width(&self.menu_heading(m, *h)))
            .max()
            .unwrap_or(0);
        (label, headings, keys)
    }

    /// The labels and keys of the items shown that can run (the headings and the dimmed items
    /// left out).
    pub fn menu_rows(&self) -> Vec<(Localized, String)> {
        let dimmed = self.menu_dimmed();
        self.menu_lines()
            .into_iter()
            .zip(dimmed)
            .filter(|((h, _, _), d)| !h && !d)
            .map(|((_, l, k), _)| (l, k))
            .collect()
    }

    /// The labels of the dimmed items shown, with the few words that say why they cannot run.
    pub fn menu_dimmed_rows(&self) -> Vec<(Localized, String)> {
        let dimmed = self.menu_dimmed();
        self.menu_lines().into_iter().zip(dimmed).filter(|(_, d)| *d).map(|((_, l, k), _)| (l, k)).collect()
    }

    /// Why the open menu's dimmed item labelled `label` cannot run, in full (what picking it says).
    pub fn menu_dimmed_notice(&self, label: &str) -> Option<Notice> {
        let m = self.overlays.menu()?;
        m.off.iter().find(|(i, _)| self.menu_row(m.ctx, &m.names, *i).0 == label).map(|(_, o)| o.notice.clone())
    }

    /// The open submenu's rows, like [`App::menu_rows`].
    pub fn sub_menu_rows(&self) -> Vec<(Localized, String)> {
        let Some(m) = self.overlays.menu() else { return Vec::new() };
        let Some(sub) = &m.sub else { return Vec::new() };
        sub.items.iter().map(|i| self.menu_row(m.ctx, &m.names, *i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::rank;

    #[test]
    fn prefix_first_then_a_word_then_letters_in_order() {
        assert_eq!(rank("cop", &["Copy the cell"]), Some(0));
        assert_eq!(rank("COP", &["copy the cell"]), Some(0), "case is ignored");
        assert_eq!(rank("cell", &["Copy the cell"]), Some(1));
        assert_eq!(rank("the c", &["Copy the cell"]), Some(1), "spaces in the text are kept");
        assert_eq!(rank("cpc", &["Copy the cell"]), Some(2));
        assert_eq!(rank("ell", &["Copy the cell"]), Some(2), "inside a word: letters in order");
        assert_eq!(rank("xyz", &["Copy the cell"]), None);
        assert_eq!(rank("", &["anything"]), Some(0));
        assert_eq!(rank("grid.c", &["View cell", "grid.copy_cell"]), Some(3), "another text: below the label");
        assert_eq!(rank("t", &["Close tab", "Close tab", "tab.close"]), Some(1), "the label's word first");
        assert_eq!(rank("xyz", &["Copy", "xyz.copy"]), Some(3));
    }
}
