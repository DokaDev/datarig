//! Quick connect (`Ctrl+O`, `:conn`): a fuzzy list of the
//! profiles. Picking one goes to its most recently used tab, or connects and opens a console
//! when it has none. The same list picks the connection of a tab (`Space c s`, a statement run
//! in a tab without one).
//!
//! The list has two levels more. `→` on a profile lists the databases of its
//! server (connecting it first when it is not), `→` on a database its schemas, `←` goes back.
//! `Enter` on a profile keeps the profile's defaults; on a database or a schema it opens (or
//! binds) a console there. Typing filters every row shown.

use super::*;
use crate::widgets::text_input::InputResult;
use datarig_core::driver::SessionContext;
use std::collections::BTreeSet;

/// What picking a profile does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickPurpose {
    /// Its most recent tab, else connect and open a console (focus in the editor).
    Open,
    /// Always a new console tab (`Ctrl+T` without a profile to use).
    NewConsole,
    /// Tab `tab` runs on the picked profile from now on; `run`: then run these statements.
    Bind { tab: TabId, run: Option<Vec<String>> },
}

/// Why a level of the list has no rows to pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickNote {
    /// Asked for; the answer has not come yet (or the profile is connecting).
    Loading,
    /// It could not be read: why.
    Failed(String),
    /// Read, and there is nothing.
    Empty,
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickRow {
    Profile(ProfileId),
    /// A database of the profile's server.
    Database(ProfileId, String),
    /// A schema of a database.
    Schema(ProfileId, String, String),
    /// What a level says instead of rows: under a profile (`None`) or a database.
    Note(ProfileId, Option<String>, QuickNote),
}

impl QuickRow {
    pub fn profile(&self) -> ProfileId {
        match self {
            QuickRow::Profile(p) | QuickRow::Database(p, _) | QuickRow::Schema(p, _, _) | QuickRow::Note(p, _, _) => *p,
        }
    }

    /// How deep it is drawn (profiles at 0).
    pub fn depth(&self) -> usize {
        match self {
            QuickRow::Profile(_) => 0,
            QuickRow::Database(..) | QuickRow::Note(_, None, _) => 1,
            QuickRow::Schema(..) | QuickRow::Note(_, Some(_), _) => 2,
        }
    }
}

pub struct QuickConnect {
    pub input: TextInput,
    /// The rows shown, in order.
    pub items: Vec<QuickRow>,
    pub selected: usize,
    pub purpose: QuickPurpose,
    /// Profiles whose databases are listed.
    pub open: BTreeSet<ProfileId>,
    /// Databases whose schemas are listed.
    pub open_db: BTreeSet<(ProfileId, String)>,
    /// Picking only the database and schema of this profile (`Space c d`, `:use`): the title
    /// says so.
    pub context_of: Option<ProfileId>,
    /// The row the cursor goes to once it is listed (where the tab works now).
    pub want: Option<QuickRow>,
}

impl App {
    /// Open the quick connect list for `purpose`.
    pub(super) fn open_quick(&mut self, purpose: QuickPurpose) {
        if self.layout.too_small || self.profiles.is_empty() {
            return;
        }
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::QuickConnect(QuickConnect {
            input: TextInput::default(),
            items: Vec::new(),
            selected: 0,
            purpose,
            open: BTreeSet::new(),
            open_db: BTreeSet::new(),
            context_of: None,
            want: None,
        }));
        self.refresh_quick();
    }

    /// `Space c d`: the list on tab `tab`'s profile, its databases listed and the
    /// tab's database open, the cursor on where the tab works now. Picking binds the tab there.
    pub(super) fn open_context_picker(&mut self, tab: TabId) {
        let Some(t) = self.tabs.get(tab) else { return };
        let Some(p) = t.profile else { return self.open_quick(QuickPurpose::Bind { tab, run: None }) };
        let db = t.context.database.clone().unwrap_or_else(|| self.own_database(p));
        let schema = t.context.schema.clone();
        self.open_quick(QuickPurpose::Bind { tab, run: None });
        if let Some(q) = self.overlays.quick_mut() {
            q.open.insert(p);
            q.open_db.insert((p, db.clone()));
            q.context_of = Some(p);
        }
        if let Some(q) = self.overlays.quick_mut() {
            q.want = Some(match schema {
                Some(s) => QuickRow::Schema(p, db.clone(), s),
                None => QuickRow::Database(p, db),
            });
        }
        self.expand_profile(p);
        self.refresh_quick();
    }

    /// The database profile `p` connects to by itself (its settings, else what its metadata
    /// session said).
    pub fn own_database(&self, p: ProfileId) -> String {
        let db = self.profile(p).map(|c| c.endpoint().1).unwrap_or_default();
        if db.is_empty() { self.conns.get(p).and_then(|c| c.database.clone()).unwrap_or_default() } else { db }
    }

    /// The profiles matching `query` (fuzzy over the name and `folder/name`), best first; all
    /// of them in explorer order (folder, then name) for an empty query.
    pub fn quick_items(&self, query: &str) -> Vec<ProfileId> {
        let key = |p: &ConnectionConfig| (p.folder.clone().unwrap_or_default().to_lowercase(), p.name.to_lowercase());
        let mut all: Vec<&ConnectionConfig> = self.profiles.iter().collect();
        all.sort_by_key(|p| key(p));
        let mut hits: Vec<(i32, usize, ProfileId)> = all
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let path = match &p.folder {
                    Some(f) => format!("{f}/{}", p.name),
                    None => p.name.clone(),
                };
                [p.name.as_str(), path.as_str()]
                    .iter()
                    .filter_map(|t| action::fuzzy_score(query, t))
                    .max()
                    .map(|s| (s, i, p.id))
            })
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.into_iter().map(|(_, _, id)| id).collect()
    }

    /// The databases of profile `p` for the list, or what to say instead.
    fn quick_databases(&self, p: ProfileId) -> Result<Vec<String>, QuickNote> {
        match self.conns.get(p).and_then(|c| c.databases.clone()) {
            Some(Ok(v)) if v.is_empty() => Err(QuickNote::Empty),
            Some(Ok(v)) => Ok(v),
            Some(Err(e)) => Err(QuickNote::Failed(e)),
            None if self.conns.state(p) == NodeState::Failed => Err(QuickNote::Failed(
                self.conns
                    .get(p)
                    .and_then(|c| c.error.as_ref())
                    .map(|n| n.render(&self.i18n).to_string())
                    .unwrap_or_default(),
            )),
            None => Err(QuickNote::Loading),
        }
    }

    /// The schemas of database `db` of profile `p` for the list: the profile's own database's
    /// from its schema tree, another's from its own metadata session.
    fn quick_schemas(&self, p: ProfileId, db: &str) -> Result<Vec<String>, QuickNote> {
        let list = if db == self.own_database(p) {
            let Some(c) = self.conns.get(p).filter(|c| c.connected) else { return Err(QuickNote::Loading) };
            if c.tree.schemas_loading {
                return Err(QuickNote::Loading);
            }
            c.tree.schemas.iter().map(|s| s.name.clone()).collect()
        } else {
            match self.conns.aux(p, db).and_then(|a| a.schemas.clone()) {
                Some(Ok(v)) => v,
                Some(Err(e)) => return Err(QuickNote::Failed(e)),
                None => return Err(QuickNote::Loading),
            }
        };
        if list.is_empty() { Err(QuickNote::Empty) } else { Ok(list) }
    }

    /// Work out the list's rows again (the filter, what is open, what has arrived) and ask
    /// for what an open level still needs. The selection stays on its row when it can.
    pub(super) fn refresh_quick(&mut self) {
        let Some(q) = self.overlays.quick() else { return };
        let query = q.input.text().to_string();
        let (open, open_db) = (q.open.clone(), q.open_db.clone());
        let before = q.items.get(q.selected).cloned();
        let only = q.context_of;
        for &(p, ref db) in &open_db {
            if open.contains(&p) && *db != self.own_database(p) {
                self.ensure_aux(p, db);
            }
        }
        let matches = |t: &str| query.trim().is_empty() || action::fuzzy_score(&query, t).is_some();
        let mut rows = Vec::new();
        let profiles: Vec<ProfileId> = match only {
            Some(p) => vec![p],
            None => self.quick_items(""),
        };
        let direct: BTreeSet<ProfileId> = self.quick_items(&query).into_iter().collect();
        for p in profiles {
            let name = self.profile(p).map(|c| c.name.clone()).unwrap_or_default();
            let mut children = Vec::new();
            if open.contains(&p) {
                match self.quick_databases(p) {
                    Err(note) => children.push(QuickRow::Note(p, None, note)),
                    Ok(dbs) => {
                        for db in dbs {
                            let mut below = Vec::new();
                            if open_db.contains(&(p, db.clone())) {
                                match self.quick_schemas(p, &db) {
                                    Err(note) => below.push(QuickRow::Note(p, Some(db.clone()), note)),
                                    Ok(schemas) => below.extend(
                                        schemas
                                            .into_iter()
                                            .filter(|s| matches(&format!("{name}/{db}/{s}")))
                                            .map(|s| QuickRow::Schema(p, db.clone(), s)),
                                    ),
                                }
                            }
                            let hit = matches(&format!("{name}/{db}"));
                            if hit || below.iter().any(|r| matches!(r, QuickRow::Schema(..))) {
                                children.push(QuickRow::Database(p, db));
                                children.extend(below);
                            }
                        }
                    }
                }
            }
            let hit = only.is_some() || query.trim().is_empty() || direct.contains(&p);
            if hit || children.iter().any(|r| matches!(r, QuickRow::Database(..))) {
                rows.push(QuickRow::Profile(p));
                rows.extend(children);
            }
        }
        // Filtering puts the best profile match first, as before the list had levels.
        if !query.trim().is_empty() && only.is_none() {
            let order = self.quick_items(&query);
            let rank = |p: ProfileId| order.iter().position(|x| *x == p).unwrap_or(usize::MAX);
            let mut groups: Vec<Vec<QuickRow>> = Vec::new();
            for r in rows {
                match r {
                    QuickRow::Profile(_) => groups.push(vec![r]),
                    _ => {
                        if let Some(g) = groups.last_mut() {
                            g.push(r)
                        }
                    }
                }
            }
            groups.sort_by_key(|g| rank(g[0].profile()));
            rows = groups.into_iter().flatten().collect();
        }
        let Some(q) = self.overlays.quick_mut() else { return };
        q.selected =
            before.and_then(|b| rows.iter().position(|r| *r == b)).unwrap_or(0).min(rows.len().saturating_sub(1));
        if let Some(i) = q.want.as_ref().and_then(|w| rows.iter().position(|r| r == w)) {
            q.selected = i;
            q.want = None;
        }
        q.items = rows;
    }

    /// List profile `p`'s databases: connect it first when it is not (the list fills in once it
    /// is), else ask its metadata session.
    fn expand_profile(&mut self, p: ProfileId) {
        match self.conns.state(p) {
            NodeState::Connected => {
                let c = self.conns.entry(p);
                if !matches!(c.databases, Some(Ok(_))) {
                    c.databases = None;
                    self.send_meta(p, DbCommand::LoadDatabases);
                }
            }
            NodeState::Connecting => {}
            NodeState::Disconnected | NodeState::Failed => self.connect(p),
        }
    }

    /// Profile `p` connected: an open list that waits for its databases asks for them.
    pub(super) fn quick_connected(&mut self, p: ProfileId) {
        if self.overlays.quick().is_some_and(|q| q.open.contains(&p)) {
            self.conns.entry(p).databases = None;
            self.send_meta(p, DbCommand::LoadDatabases);
        }
    }

    /// Keys of the quick connect list (`overlay.quick_connect`, text input).
    pub(super) fn quick_key(&mut self, key: KeyEvent, repeat: bool) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(q) = self.overlays.quick_mut() else { return };
        let n = q.items.len().max(1);
        let row = q.items.get(q.selected).cloned();
        q.want = None;
        match key.code {
            KeyCode::Esc => self.overlays.close(OverlayKind::QuickConnect),
            KeyCode::Enter if !repeat => {
                let Some(q) = self.overlays.quick() else { return };
                let purpose = q.purpose.clone();
                let Some(row) = row else { return };
                let (p, context) = match row {
                    QuickRow::Profile(p) => (p, None),
                    QuickRow::Database(p, db) => (p, Some(self.session_context(p, Some(db), None))),
                    QuickRow::Schema(p, db, s) => (p, Some(self.session_context(p, Some(db), Some(s)))),
                    // Nothing to pick on a note; `→` on its level asks again.
                    QuickRow::Note(..) => return,
                };
                // `Space c d` keeps the tab's profile: Enter on it keeps its defaults.
                let context = context.or_else(|| q.context_of.map(|_| SessionContext::default()));
                self.overlays.close(OverlayKind::QuickConnect);
                self.quick_choose(p, purpose, context);
            }
            KeyCode::Enter => {}
            KeyCode::Up => q.selected = (q.selected + n - 1) % n,
            KeyCode::Char('p') if ctrl => q.selected = (q.selected + n - 1) % n,
            KeyCode::Down => q.selected = (q.selected + 1) % n,
            KeyCode::Char('n') if ctrl => q.selected = (q.selected + 1) % n,
            KeyCode::Right if !repeat => {
                match row {
                    Some(QuickRow::Profile(p)) => {
                        q.open.insert(p);
                        self.expand_profile(p);
                    }
                    Some(QuickRow::Database(p, db)) => {
                        q.open_db.insert((p, db));
                    }
                    Some(QuickRow::Note(p, None, QuickNote::Failed(_))) => self.expand_profile(p),
                    _ => {}
                }
                self.refresh_quick();
            }
            KeyCode::Left if !repeat => {
                let parent = |q: &QuickConnect, want: &dyn Fn(&QuickRow) -> bool| {
                    q.items[..q.selected.min(q.items.len())].iter().rposition(want)
                };
                match row {
                    Some(QuickRow::Profile(p)) => {
                        q.open.remove(&p);
                    }
                    Some(QuickRow::Database(p, db)) if q.open_db.contains(&(p, db.clone())) => {
                        q.open_db.remove(&(p, db));
                    }
                    Some(QuickRow::Database(..) | QuickRow::Note(_, None, _)) => {
                        if let Some(i) = parent(q, &|r| matches!(r, QuickRow::Profile(_))) {
                            q.selected = i;
                        }
                    }
                    Some(QuickRow::Schema(..) | QuickRow::Note(_, Some(_), _)) => {
                        if let Some(i) = parent(q, &|r| matches!(r, QuickRow::Database(..))) {
                            q.selected = i;
                        }
                    }
                    None => {}
                }
                self.refresh_quick();
            }
            _ => {
                if q.input.handle_key(&key) == InputResult::Changed {
                    if let Some(q) = self.overlays.quick_mut() {
                        q.selected = 0;
                        q.items.clear();
                    }
                    self.refresh_quick();
                }
            }
        }
    }

    /// The context of `db` and `schema` on profile `p`: the profile's own database is its
    /// default (`None`), so a tab there needs no metadata session of its own.
    pub(super) fn session_context(&self, p: ProfileId, db: Option<String>, schema: Option<String>) -> SessionContext {
        let own = self.own_database(p);
        SessionContext { database: db.filter(|d| *d != own), schema }
    }

    /// Profile `id` was picked for `purpose`, in `context` (`None`: a profile row, its most
    /// recent tab or its defaults).
    pub(super) fn quick_choose(&mut self, id: ProfileId, purpose: QuickPurpose, context: Option<SessionContext>) {
        match (purpose, context) {
            (QuickPurpose::Open, None) => self.goto_profile(id),
            (QuickPurpose::Open | QuickPurpose::NewConsole, Some(ctx)) => self.console_in(id, ctx),
            (QuickPurpose::NewConsole, None) => {
                self.open_console(id, true);
            }
            (QuickPurpose::Bind { tab, run }, context) => {
                self.bind_tab_in(tab, id, context.unwrap_or_default());
                if let Some(statements) = run {
                    self.run_in(tab, statements);
                } else if self.conns.state(id) != NodeState::Connected {
                    self.connect(id);
                }
            }
        }
        self.reveal_profile(id);
    }

    /// A new console on profile `id` in `context`, focused; the profile connects when it is not.
    pub(super) fn console_in(&mut self, id: ProfileId, context: SessionContext) {
        let tab = self.open_console(id, true);
        self.tabs.bind_in(tab, Some(id), context);
        self.save_workspace();
        if self.conns.state(id) == NodeState::Disconnected {
            self.connect(id);
        }
    }

    /// Go to profile `id`: its most recently used tab if it has one, otherwise connect
    /// and open a console tab. The editor gets the focus.
    pub(super) fn goto_profile(&mut self, id: ProfileId) {
        if let Some(t) = self.tabs.recent_for(id) {
            self.switch_tab(|m| m.position(t).is_some_and(|i| m.activate(i)));
            self.focus = Focus::Editor;
            return;
        }
        if self.conns.is_connected(id) {
            self.open_console(id, true);
            return;
        }
        self.conns.entry(id).console = Some(true);
        if self.conns.state(id) != NodeState::Connecting {
            self.connect(id);
        }
    }
}
