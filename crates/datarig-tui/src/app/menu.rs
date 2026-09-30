//! Context menus (right click). The explorer's: the explorer's
//! actions that apply to the node under the pointer, with their keys. It is built from the
//! explorer's key bindings (the same list the keyboard help shows for the explorer), so an
//! action added to the explorer shows up here too. Moving and quitting are left out; what
//! cannot run on the node is left out as well. The grid's: the cell viewer,
//! the inspector, copying the cell or row and "copy as" each format, for the cell under the
//! pointer. Each menu shows the keys of its own context.

use super::copy::{CopyFormat, CopyScope};
use super::*;
use crate::keymap::{Target, keys};

/// Explorer actions that are not about a node (moving the cursor, the filter, quitting):
/// never in the menu.
const NOT_IN_MENU: &[Action] = &[
    Action::Explorer(ExplorerAction::Down),
    Action::Explorer(ExplorerAction::Up),
    Action::Explorer(ExplorerAction::Top),
    Action::Explorer(ExplorerAction::Bottom),
    Action::Explorer(ExplorerAction::Expand),
    Action::Explorer(ExplorerAction::Collapse),
    Action::Explorer(ExplorerAction::Filter),
    Action::Explorer(ExplorerAction::FilterClear),
    Action::Explorer(ExplorerAction::FilterAccept),
    Action::Explorer(ExplorerAction::Back),
    Action::Explorer(ExplorerAction::ContextMenu),
    Action::Quit,
];

/// The actions of the grid's menu, in order; the copy scopes follow them.
const GRID_MENU: &[Action] = &[
    Action::Grid(GridAction::ViewCell),
    Action::ToggleDetail,
    Action::Grid(GridAction::CopyCell),
    Action::Grid(GridAction::CopyRow),
];

/// What a copy of the grid's menu takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuScope {
    Copy(CopyScope),
    /// Every row: the rest is fetched first (asked first), then every row is copied.
    FetchFirst,
}

/// A row of a context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Action(Action),
    /// Opens the formats of a copy scope (the second level).
    Scope(MenuScope),
    /// A format of a scope's submenu.
    Copy(MenuScope, CopyFormat),
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
    pub items: Vec<MenuItem>,
    pub selected: usize,
    /// Where it was opened (its top-left corner, clamped by the renderer).
    pub at: (u16, u16),
    /// Screen area of the items, kept by the renderer (mouse).
    pub list: Rect,
    /// The open submenu, which has the keys.
    pub sub: Option<SubMenu>,
}

impl ContextMenu {
    fn new(ctx: Ctx, items: Vec<MenuItem>, at: (u16, u16)) -> Self {
        Self { ctx, items, selected: 0, at, list: Rect::default(), sub: None }
    }

    /// The actions of its rows (not the scopes).
    pub fn actions(&self) -> Vec<Action> {
        self.items.iter().filter_map(|i| if let MenuItem::Action(a) = i { Some(*a) } else { None }).collect()
    }
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
        match a {
            Action::Explorer(ExplorerAction::Activate) => !matches!(row, R::NewConnection | R::ScriptsEmpty),
            Action::Explorer(ExplorerAction::Refresh) => connected || script || matches!(row, R::ScriptsHeader),
            Action::Explorer(ExplorerAction::Delete) => matches!(row, R::Profile(_) | R::Folder(_)) || script,
            Action::Explorer(ExplorerAction::Rename) => matches!(row, R::Folder(_)) || script,
            Action::Explorer(ExplorerAction::Move) => profile.is_some() || script,
            Action::Explorer(ExplorerAction::ConsoleHere) => in_database,
            // In a database or schema the console opens there (`O`), not with the profile's
            // defaults: one console item, which says where.
            Action::OpenConsole => profile.is_some() && !in_database,
            Action::EditProfile | Action::DuplicateProfile | Action::TestConnection => profile.is_some(),
            Action::Disconnect => profile.is_some_and(|id| self.conns.state(id) != NodeState::Disconnected),
            // Profile and folder actions of the explorer have nothing to do on the scripts.
            Action::NewProfile | Action::Explorer(ExplorerAction::NewFolder) => {
                !script && !matches!(row, R::ScriptsHeader | R::ScriptsEmpty)
            }
            _ => true,
        }
    }

    /// The menu's actions for the row under the cursor, in the explorer's binding order.
    pub fn menu_items(&self) -> Vec<Action> {
        let Some(row) = self.explorer_row() else { return Vec::new() };
        let mut out: Vec<Action> = Vec::new();
        for e in self.keymap.section(Ctx::Explorer, Some(Ctx::Explorer)) {
            let a = e.action;
            if !out.contains(&a)
                && !NOT_IN_MENU.contains(&a)
                && (action::spec(a).when)(self)
                && self.applies(a, &row.kind)
            {
                out.push(a);
            }
        }
        out
    }

    /// The label of explorer action `a` in the menu, for the row it runs on (the cursor's):
    /// what it deletes, moves or renames there, and where the console opens. `None`: the
    /// action's own label.
    fn explorer_menu_label(&self, a: Action) -> Option<Localized> {
        use explorer::RowKind as R;
        let row = self.explorer_row()?;
        let label = match (a, &row.kind) {
            (Action::Explorer(ExplorerAction::ConsoleHere), _) => {
                let (id, ctx) = self.console_here_context(&row)?;
                let place = match (ctx.database, ctx.schema) {
                    (Some(db), Some(schema)) => format!("{db}.{schema}"),
                    (None, Some(schema)) => schema,
                    (Some(db), None) => db,
                    (None, None) => self.own_database(id),
                };
                return Some(self.i18n.msg(&Msg::MenuConsoleIn { place }));
            }
            (Action::Explorer(ExplorerAction::Rename), R::Script(_)) => Label::MenuRenameQuery,
            (Action::Explorer(ExplorerAction::Rename), R::Folder(_) | R::ScriptFolder(_)) => Label::MenuRenameFolder,
            (Action::Explorer(ExplorerAction::Delete), R::Script(_)) => Label::MenuDeleteQuery,
            (Action::Explorer(ExplorerAction::Delete), R::Folder(_) | R::ScriptFolder(_)) => Label::MenuDeleteFolder,
            (Action::Explorer(ExplorerAction::Delete), R::Profile(_)) => Label::MenuDeleteProfile,
            (Action::Explorer(ExplorerAction::Move), R::Script(_)) => Label::MenuMoveQuery,
            (Action::Explorer(ExplorerAction::Move), R::ScriptFolder(_)) => Label::MenuMoveFolder,
            (Action::Explorer(ExplorerAction::Move), _) => Label::MenuMoveProfile,
            _ => return None,
        };
        Some(self.i18n.label(label))
    }

    /// Right click on the explorer at (x, y): select the row there (blank space: the
    /// "＋ New connection" row) and open the menu.
    pub(super) fn open_context_menu(&mut self, x: u16, y: u16) {
        self.focus = Focus::Tree;
        self.explorer.filtering = false;
        let rows = self.explorer_rows();
        let area = self.explorer.area;
        let i = if y <= area.y { 0 } else { 1 + self.explorer.scroll + usize::from(y - area.y - 1) };
        self.explorer.select(&rows, if i < rows.len() { i } else { 0 });
        let items = self.menu_items();
        if items.is_empty() {
            return;
        }
        let items = items.into_iter().map(MenuItem::Action).collect();
        self.overlays.push(Overlay::ContextMenu(ContextMenu::new(Ctx::Explorer, items, (x, y))));
    }

    /// Right click on the grid at (x, y): on the selected range, the menu for that range;
    /// elsewhere the cell there is selected (the range dropped) and the menu is for it.
    pub(super) fn open_grid_menu(&mut self, x: u16, y: u16) {
        self.focus = Focus::Results;
        let t = self.tabs.active_mut();
        let Results::Rows(rs) = &t.results else { return };
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let on_range = t.grid.cell_at(x, y, total).is_some_and(|c| t.grid.in_range(c, total, ncols));
        if !on_range && t.grid.click(x, y, rs.rows.len()) {
            t.grid.anchor = None;
        }
        let mut items: Vec<MenuItem> =
            GRID_MENU.iter().copied().filter(|a| (action::spec(*a).when)(self)).map(MenuItem::Action).collect();
        if matches!(&self.tab().results, Results::Rows(rs) if !rs.rows.is_empty()) {
            items.push(MenuItem::Scope(MenuScope::Copy(CopyScope::Selection)));
            items.push(MenuItem::Scope(MenuScope::Copy(CopyScope::Fetched)));
            if self.can_fetch_rest() {
                items.push(MenuItem::Scope(MenuScope::FetchFirst));
            }
        }
        if !items.is_empty() {
            self.overlays.push(Overlay::ContextMenu(ContextMenu::new(Ctx::Grid, items, (x, y))));
        }
    }

    /// Open the formats of the scope of row `i` of the menu.
    fn open_sub(&mut self, i: usize) {
        let Some(m) = self.overlays.menu_mut() else { return };
        let Some(MenuItem::Scope(scope)) = m.items.get(i).copied() else { return };
        m.selected = i;
        let items = CopyFormat::MENU.iter().map(|f| MenuItem::Copy(scope, *f)).collect();
        m.sub = Some(SubMenu { scope, items, selected: 0, list: Rect::default() });
    }

    /// `explorer.context_menu` from a key or the command line: the menu at the cursor's row.
    pub(super) fn open_context_menu_here(&mut self) {
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        let area = self.explorer.area;
        let y = if i == 0 { area.y } else { area.y + 1 + (i - 1).saturating_sub(self.explorer.scroll) as u16 };
        self.open_context_menu(area.x + 4, y.min(area.y + area.height.saturating_sub(1)));
    }

    /// Run `item` of the menu (it closes first), or open a scope's formats.
    fn menu_run(&mut self, item: MenuItem) {
        match item {
            MenuItem::Scope(_) => {
                let i = self.overlays.menu().and_then(|m| m.items.iter().position(|x| *x == item)).unwrap_or(0);
                self.open_sub(i);
            }
            MenuItem::Action(a) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.dispatch(a);
            }
            MenuItem::Copy(MenuScope::Copy(scope), f) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.dispatch(Action::Copy(scope, f));
            }
            MenuItem::Copy(MenuScope::FetchFirst, f) => {
                self.overlays.close(OverlayKind::ContextMenu);
                self.ask_fetch_then_copy(f);
            }
        }
    }

    /// Keys of the menu (`overlay.context_menu`): move, `Enter` runs (or opens a scope's
    /// formats, as `l`/`→` do), `Esc`/`q` close, and the key shown next to an item runs it too.
    /// In an open submenu the arrows move, `Enter`/`→` run, `Esc`/`←` go back to the first level
    /// and a format's letter (`t`, `c`, `j`, …) runs that format.
    pub(super) fn menu_key(&mut self, k: KeyChord, repeat: bool) {
        let Some(m) = self.overlays.menu_mut() else { return };
        let plain = k.mods.is_empty() || k.mods == KeyModifiers::SHIFT;
        if let Some(sub) = m.sub.as_mut() {
            let n = sub.items.len().max(1);
            // Letters are the formats' keys here (`j` is JSON): the arrows move.
            match k.code {
                KeyCode::Down if plain => sub.selected = (sub.selected + 1) % n,
                KeyCode::Up if plain => sub.selected = (sub.selected + n - 1) % n,
                _ if repeat => {}
                KeyCode::Esc | KeyCode::Left => m.sub = None,
                KeyCode::Enter | KeyCode::Right => {
                    let item = sub.items[sub.selected.min(sub.items.len() - 1)];
                    self.menu_run(item);
                }
                // A format's key (as after `Space r y`).
                KeyCode::Char(c) => {
                    if let Some(item) =
                        sub.items.iter().copied().find(|i| matches!(i, MenuItem::Copy(_, f) if f.key() == c))
                    {
                        self.menu_run(item);
                    }
                }
                _ => {}
            }
            return;
        }
        let n = m.items.len().max(1);
        match k.code {
            KeyCode::Char('j') | KeyCode::Down if k.mods.is_empty() => m.selected = (m.selected + 1) % n,
            KeyCode::Char('k') | KeyCode::Up if k.mods.is_empty() => m.selected = (m.selected + n - 1) % n,
            _ if repeat => {}
            KeyCode::Esc | KeyCode::Char('q') => self.overlays.close(OverlayKind::ContextMenu),
            KeyCode::Enter => {
                let item = m.items.get(m.selected).copied();
                if let Some(item) = item {
                    self.menu_run(item);
                }
            }
            KeyCode::Char('l') | KeyCode::Right if matches!(m.items.get(m.selected), Some(MenuItem::Scope(_))) => {
                let i = m.selected;
                self.open_sub(i);
            }
            _ => {
                let ctx = m.ctx;
                if let (Some(Target::Action(a)), _) = self.keymap.resolve_seq(ctx, &[k])
                    && self.overlays.menu().is_some_and(|m| m.items.contains(&MenuItem::Action(a)))
                {
                    self.menu_run(MenuItem::Action(a));
                }
            }
        }
    }

    /// Mouse while the menu is open: a click on an item runs it (on a scope: opens its
    /// formats), anywhere else closes it; the wheel moves the selection.
    pub(super) fn menu_mouse(&mut self, ev: MouseEvent) {
        let Some(m) = self.overlays.menu_mut() else { return };
        let at = ratatui::layout::Position::new(ev.column, ev.row);
        match ev.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = ev.kind == MouseEventKind::ScrollDown;
                let (sel, n) = match m.sub.as_mut() {
                    Some(sub) => (&mut sub.selected, sub.items.len().max(1)),
                    None => (&mut m.selected, m.items.len().max(1)),
                };
                *sel = if down { (*sel + 1) % n } else { (*sel + n - 1) % n };
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
                    let item = m.items.get(usize::from(ev.row - m.list.y)).copied();
                    m.sub = None;
                    if let Some(item) = item {
                        self.menu_run(item);
                    }
                } else {
                    self.overlays.close(OverlayKind::ContextMenu);
                }
            }
            _ => {}
        }
    }

    /// The pointer moved over the open menu (no button): the item under it is selected, as the
    /// keys would, and the keys go on from there. Over another row of the menu than the one whose
    /// formats are open, the formats close. `false` when nothing changed (off the items, or on
    /// the selected one): no frame is drawn for it.
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
        if !m.list.contains(at) {
            return false;
        }
        let i = usize::from(ev.row - m.list.y);
        if i >= m.items.len() || i == m.selected {
            return false;
        }
        m.selected = i;
        m.sub = None;
        true
    }

    /// The label and keys of a menu row (keys in the menu's context).
    fn menu_row(&self, ctx: Ctx, item: MenuItem) -> (Localized, String) {
        let keys =
            |a: Action| self.keymap.hint_keys(a, ctx, self.enhanced_keys).map(|k| keys::label(&k)).unwrap_or_default();
        match item {
            MenuItem::Action(a) => {
                let own = (ctx == Ctx::Explorer).then(|| self.explorer_menu_label(a)).flatten();
                (own.unwrap_or_else(|| self.i18n.label(action::spec(a).label)), keys(a))
            }
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
                };
                // The mark of a row that opens more rows.
                (Localized::verbatim(format!("{label} ▸")), String::new())
            }
            // In the submenu a format's own key runs it (in both scopes): that key is shown.
            MenuItem::Copy(_, f) => (self.i18n.label(f.menu_label()), f.key().to_string()),
        }
    }

    /// The menu's rows: the label and its keys in the menu's context.
    pub fn menu_rows(&self) -> Vec<(Localized, String)> {
        let Some(m) = self.overlays.menu() else { return Vec::new() };
        m.items.iter().map(|i| self.menu_row(m.ctx, *i)).collect()
    }

    /// The open submenu's rows, like [`App::menu_rows`].
    pub fn sub_menu_rows(&self) -> Vec<(Localized, String)> {
        let Some(m) = self.overlays.menu() else { return Vec::new() };
        let Some(sub) = &m.sub else { return Vec::new() };
        sub.items.iter().map(|i| self.menu_row(m.ctx, *i)).collect()
    }
}
