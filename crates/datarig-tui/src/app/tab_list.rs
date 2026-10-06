//! The tab list (`Space t t`, `:tabs`, `:ls`, `:buffers`, the action menu): every open tab,
//! the most recently active first, then the tabs closed in this run, newest first, under a
//! heading of their own. The active tab comes first and the one active before it is selected,
//! so `Enter` goes back to it.
//!
//! Typing filters both parts by a tab's name, its profile, database and schema, its kind and
//! its number, ranked as the action menu ranks its items (a name that starts with the text,
//! then a word of it, then its letters in order). `Enter` goes to the selected tab, or brings
//! a closed one back (`Space t u`'s data, that entry rather than the newest). `Ctrl+D` closes
//! the selected tab as `Ctrl+W` closes the active one, its confirmations included; the list
//! stays open and follows.

use super::overlay::Press;
use super::tabs::ClosedTab;
use super::*;
use crate::widgets::text_input::InputResult;
use datarig_core::driver::SessionContext;

/// An entry of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEntry {
    /// An open tab.
    Open(TabId),
    /// A tab closed in this run, by its [`ClosedTab::serial`].
    Closed(u64),
}

pub struct TabList {
    pub filter: TextInput,
    /// The entries shown: the open tabs that match, then the closed ones.
    pub entries: Vec<TabEntry>,
    pub selected: usize,
    /// The first line shown (the closed tabs' heading is a line too), kept by the renderer.
    pub scroll: usize,
    /// Where each entry on screen was drawn (mouse), kept by the renderer.
    pub rows: Vec<(Rect, usize)>,
    /// The entry under the pointer (highlighted; the selection does not move to it).
    pub hover: Option<usize>,
    /// The entry a press armed (it is picked on the release over it).
    pub press: Press<usize>,
}

impl TabList {
    /// The open tabs listed.
    pub fn open_count(&self) -> usize {
        self.entries.iter().take_while(|e| matches!(e, TabEntry::Open(_))).count()
    }

    /// Select the next (`d = 1`) or previous entry, around the ends.
    fn step(&mut self, d: isize) {
        let n = self.entries.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + d).rem_euclid(n) as usize;
        }
    }
}

/// Which entry is selected after the entries are worked out again.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Select {
    /// Just opened: the tab active before the current one (the current one when it is alone).
    Previous,
    /// The filter changed: the best match.
    First,
    /// The same entry as before (the same place when it went).
    Same,
}

impl App {
    /// `tab.list`: open the tab list.
    pub(super) fn open_tab_list(&mut self) {
        if self.layout.too_small {
            return;
        }
        if !self.tabs.is_empty() {
            let t = self.tab_mut();
            t.popup = None;
            t.completion_due = None;
        }
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::TabList(TabList {
            filter: TextInput::default(),
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            rows: Vec::new(),
            hover: None,
            press: Press::default(),
        }));
        self.fill_tab_list(Select::Previous);
    }

    /// The tabs changed (one closed or came back): the open list follows, its selection on the
    /// same entry, or in the same place when that one went.
    pub(super) fn refresh_tab_list(&mut self) {
        self.fill_tab_list(Select::Same);
    }

    fn fill_tab_list(&mut self, select: Select) {
        let Some(l) = self.overlays.tab_list() else { return };
        let query = l.filter.text().trim().to_string();
        let (before, at) = (l.entries.get(l.selected).copied(), l.selected);
        let ranked = |entries: Vec<(TabEntry, Vec<String>)>| -> Vec<TabEntry> {
            let mut hits: Vec<(u8, usize, TabEntry)> = entries
                .into_iter()
                .enumerate()
                .filter_map(|(i, (e, texts))| {
                    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
                    menu::rank(&query, &texts).map(|r| (r, i, e))
                })
                .collect();
            hits.sort_by_key(|&(r, i, _)| (r, i));
            hits.into_iter().map(|(_, _, e)| e).collect()
        };
        let open: Vec<(TabEntry, Vec<String>)> = self
            .tabs
            .by_recent()
            .into_iter()
            .filter_map(|id| self.tabs.get(id))
            .map(|t| (TabEntry::Open(t.id), self.open_texts(t)))
            .collect();
        let closed: Vec<(TabEntry, Vec<String>)> =
            self.tabs.closed().map(|c| (TabEntry::Closed(c.serial), self.closed_texts(c))).collect();
        let mut entries = ranked(open);
        let open_count = entries.len();
        entries.extend(ranked(closed));
        let Some(l) = self.overlays.tab_list_mut() else { return };
        let last = entries.len().saturating_sub(1);
        l.selected = match select {
            Select::Previous if query.is_empty() && open_count > 1 => 1,
            Select::Previous | Select::First => 0,
            Select::Same => before.and_then(|b| entries.iter().position(|e| *e == b)).unwrap_or(at).min(last),
        };
        // Other entries: the highlight under the pointer goes.
        if l.entries != entries {
            l.hover = None;
        }
        l.entries = entries;
    }

    /// What an open tab is found by: its name, its kind (as shown and in English), its profile,
    /// database and schema, and its number.
    fn open_texts(&self, t: &Tab) -> Vec<String> {
        let number = self.tabs.position(t.id).map_or(String::new(), |i| (i + 1).to_string());
        let mut texts = vec![crate::widgets::tabbar::document_name(self, t)];
        texts.extend(self.kind_texts(t.kind));
        texts.extend(self.place_texts(t.profile, &t.context));
        texts.push(number);
        texts
    }

    /// What a closed tab is found by: as an open one, without a number.
    fn closed_texts(&self, c: &ClosedTab) -> Vec<String> {
        let mut texts = vec![self.closed_name(c)];
        texts.extend(self.kind_texts(c.kind));
        texts.extend(self.place_texts(c.profile, &c.context));
        texts
    }

    fn kind_texts(&self, kind: TabKind) -> [String; 2] {
        let l = kind_label(kind);
        [self.i18n.label(l).to_string(), l.text(Lang::En).to_string()]
    }

    fn place_texts(&self, profile: Option<ProfileId>, context: &SessionContext) -> Vec<String> {
        let Some(p) = profile.and_then(|p| self.profile(p)) else { return Vec::new() };
        let mut texts = vec![p.name.clone(), self.tab_database(p.id, context)];
        texts.extend(context.schema.clone());
        texts
    }

    /// The database a tab of profile `p` works in.
    fn tab_database(&self, p: ProfileId, context: &SessionContext) -> String {
        context.database.clone().unwrap_or_else(|| self.own_database(p))
    }

    /// Where a tab works, as the list shows it: `profile · database.schema` (the schema when it
    /// picked one), `None` without a profile.
    pub fn tab_place(&self, profile: Option<ProfileId>, context: &SessionContext) -> Option<String> {
        let p = self.profile(profile?)?;
        let db = self.tab_database(p.id, context);
        let place = match (&context.schema, db.is_empty()) {
            (Some(s), true) => s.clone(),
            (Some(s), false) => format!("{db}.{s}"),
            (None, _) => db,
        };
        Some(if place.is_empty() { p.name.clone() } else { format!("{} · {place}", p.name) })
    }

    /// A closed tab's name as its tab showed it.
    pub fn closed_name(&self, c: &ClosedTab) -> String {
        if let Some(path) = &c.script {
            return datarig_core::scripts::display_name(path, false).to_string();
        }
        if let Some(name) = c.table.as_ref().map(|t| t.label()).or_else(|| c.ddl.as_ref().map(|d| d.label())) {
            return name;
        }
        self.i18n.msg(&Msg::TabConsole { n: c.console_no.to_string() }).to_string()
    }

    /// Keys of the tab list (`overlay.tab_list`, text input): typing filters it; `↓`/`↑`,
    /// `Ctrl+N`/`Ctrl+P`, `Tab`/`Shift+Tab` move; `Enter` goes to the selected tab (brings a
    /// closed one back); `Ctrl+D` closes the selected tab; `Esc` closes the list (`Backspace`
    /// on an empty filter does not).
    pub(super) fn tab_list_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(l) = self.overlays.tab_list_mut() else { return };
        let (plain, ctrl) = (key.modifiers.is_empty(), key.modifiers == KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Down | KeyCode::Tab if plain => l.step(1),
            KeyCode::Char('n') if ctrl => l.step(1),
            KeyCode::Up if plain => l.step(-1),
            KeyCode::BackTab => l.step(-1),
            KeyCode::Char('p') if ctrl => l.step(-1),
            KeyCode::Esc | KeyCode::Enter if repeat => {}
            KeyCode::Char('d') if ctrl && repeat => {}
            KeyCode::Esc if plain => self.overlays.close(OverlayKind::TabList),
            KeyCode::Enter if plain => {
                let i = l.selected;
                self.tab_list_pick(i);
            }
            KeyCode::Char('d') if ctrl => {
                if let Some(TabEntry::Open(id)) = l.entries.get(l.selected).copied() {
                    self.request_close(id);
                }
            }
            _ => {
                if l.filter.handle_key(&key) == InputResult::Changed {
                    self.fill_tab_list(Select::First);
                }
                return;
            }
        }
        // The selection or the entries moved: the highlight under the pointer goes.
        if let Some(l) = self.overlays.tab_list_mut() {
            l.hover = None;
        }
    }

    /// Text pasted while the list has the keyboard: into its filter.
    pub(super) fn tab_list_paste(&mut self, text: &str) {
        let Some(l) = self.overlays.tab_list_mut() else { return };
        l.filter.insert_str(text);
        self.fill_tab_list(Select::First);
    }

    /// Entry `i` was picked: the list closes and its tab becomes active (a closed one comes
    /// back first), the editor focused.
    fn tab_list_pick(&mut self, i: usize) {
        let Some(entry) = self.overlays.tab_list().and_then(|l| l.entries.get(i).copied()) else { return };
        self.overlays.close(OverlayKind::TabList);
        match entry {
            TabEntry::Open(id) => {
                self.switch_tab(|m| m.position(id).is_some_and(|i| m.activate(i)));
                self.focus = Focus::Editor;
            }
            TabEntry::Closed(serial) => self.reopen_closed(serial),
        }
    }

    /// The mouse on the tab list: a click on an entry picks it (as `Enter`, on the release over
    /// the entry pressed), one on the filter line puts the cursor there; the wheel moves the
    /// selection. A click outside it does nothing.
    pub(super) fn tab_list_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(l) = self.overlays.tab_list_mut() else { return };
        let at = ratatui::layout::Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown => return self.tab_list_key(KeyEvent::from(KeyCode::Down), false),
            MouseEventKind::ScrollUp => return self.tab_list_key(KeyEvent::from(KeyCode::Up), false),
            _ => {}
        }
        let row = l.rows.iter().find(|(r, _)| r.contains(at)).map(|r| r.1);
        if let Some(i) = l.press.press(m.kind, row, now) {
            l.selected = i;
            self.tab_list_pick(i);
        } else if m.kind == MouseEventKind::Down(MouseButton::Left) && row.is_none() {
            l.filter.click(m.column, m.row);
        }
    }

    /// The pointer over the tab list: the entry under it is highlighted (not over the selected
    /// one, where it would not show); `true` when that changed.
    pub(super) fn tab_list_hover(&mut self, m: MouseEvent) -> bool {
        let Some(l) = self.overlays.tab_list_mut() else { return false };
        let at = ratatui::layout::Position::new(m.column, m.row);
        let h = l.rows.iter().find(|(r, _)| r.contains(at)).map(|r| r.1).filter(|i| *i != l.selected);
        std::mem::replace(&mut l.hover, h) != h
    }
}

/// The word for a tab's kind in the list.
pub fn kind_label(kind: TabKind) -> Label {
    match kind {
        TabKind::Console => Label::TabListKindConsole,
        TabKind::Script => Label::TabListKindScript,
        TabKind::Table => Label::TabListKindTable,
        TabKind::Ddl => Label::TabListKindDdl,
    }
}
