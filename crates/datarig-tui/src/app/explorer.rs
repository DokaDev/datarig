//! The explorer: every profile, organized in
//! nested folders, with its connection state (`○ ⠋ ● ✕`), an error line under a failed node,
//! and the schema tree of a connected profile below it. It replaces the startup picker: the
//! profile keys (connect, new console, edit, duplicate, delete, test, disconnect) work here.
//! Below the profiles, the "Tunnels" section lists the tunnel presets, each with the state of
//! its shared connection and, opened, the profiles connected through it now; the same keys
//! make, edit, copy, delete and test them. The "Environments" section below it lists the environments, each
//! with its color, badge and rules and, opened, the profiles that name it; the same keys make,
//! edit, copy, rename and delete them, and while there is none a row adds the presets.
//!
//! The rows are built from the profiles, the folders and the connections whenever they are
//! needed ([`App::explorer_rows`]). The cursor remembers its row, not its index, so it stays on
//! the same node while rows appear or disappear around it (a schema list arriving for another
//! profile, an error line).

use super::*;
use crate::widgets::tree::{Node, Reveal, Tree};
use datarig_core::driver::SessionContext;
use datarig_core::profile::folder::FolderPath;
use datarig_core::sql::complete::Catalog;

/// What a row of the explorer shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// "＋ New connection", always the first row.
    NewConnection,
    /// "Connections", the head of the folders' and profiles' section.
    ConnectionsHeader,
    Folder(FolderPath),
    Profile(ProfileId),
    /// The error of the profile's last attempt, under its node.
    ProfileError(ProfileId),
    /// A database of the profile's server: its own (`None`, listed first and marked
    /// `default`) or another one.
    Database(ProfileId, Option<String>),
    /// The server's databases are being read, or why they could not be (instead of the other
    /// databases' rows).
    DatabasesNote(ProfileId),
    /// Why another database cannot be read (its only child: never shown as empty).
    DatabaseNote(ProfileId, String),
    /// A node of the schema tree of the profile's own database.
    Node(ProfileId, Node),
    /// A node of the schema tree of another database (its aux metadata session's).
    AuxNode(ProfileId, String, Node),
    /// "Saved queries", the head of the scripts section.
    ScriptsHeader,
    /// A folder of the saved queries (its path).
    ScriptFolder(String),
    /// A saved query (its path).
    Script(String),
    /// The section is open and empty: how to save one.
    ScriptsEmpty,
    /// "Tunnels", the head of the tunnel presets' section.
    TunnelsHeader,
    /// A tunnel preset.
    Tunnel(datarig_core::profile::tunnel::TunnelId),
    /// Why the preset's last connection ended, under it.
    TunnelError(datarig_core::profile::tunnel::TunnelId),
    /// A profile connected through the preset now, under it (opened).
    TunnelUser(datarig_core::profile::tunnel::TunnelId, ProfileId),
    /// The section is open and has no preset: how to make one.
    TunnelsEmpty,
    /// "Environments", the head of the environments' section.
    EnvironmentsHeader,
    /// The default rules (no environment), first in the section.
    DefaultRules,
    /// An environment.
    Environment(datarig_core::policy::EnvironmentId),
    /// A profile that names the environment, under it (opened).
    EnvironmentUser(datarig_core::policy::EnvironmentId, ProfileId),
    /// A profile without an environment, under the default rules (opened).
    DefaultRulesUser(ProfileId),
    /// The section is open and has no environment: `Enter` adds the presets.
    EnvironmentPresets,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub depth: usize,
    /// The `/` filter shows it open for a match below it, whatever its own expansion (which
    /// stays as it is): its arrow is open.
    pub opened: bool,
    /// Its own name (or a profile's host or database) matches the `/` filter.
    pub hit: bool,
}

impl Row {
    /// A row of `kind` at `depth`, as its own expansion shows it.
    pub fn new(kind: RowKind, depth: usize) -> Self {
        Self { kind, depth, opened: false, hit: false }
    }

    /// A row the filter shows open for a match below it.
    fn opened(kind: RowKind, depth: usize) -> Self {
        Self { opened: true, ..Self::new(kind, depth) }
    }

    /// A row whose name matches the filter.
    fn hit(kind: RowKind, depth: usize) -> Self {
        Self { hit: true, ..Self::new(kind, depth) }
    }

    /// The profile the row belongs to.
    pub fn profile(&self) -> Option<ProfileId> {
        match self.kind {
            RowKind::Profile(id)
            | RowKind::ProfileError(id)
            | RowKind::Database(id, _)
            | RowKind::DatabasesNote(id)
            | RowKind::DatabaseNote(id, _)
            | RowKind::Node(id, _)
            | RowKind::AuxNode(id, _, _) => Some(id),
            _ => None,
        }
    }
}

/// Explorer state: the cursor, the scroll position and the `/` filter.
pub struct Explorer {
    cursor: RowKind,
    /// Index of the cursor when it was last placed (where it goes when its row disappears).
    last: usize,
    /// First scrolled row shown below the pinned first row, kept by the renderer.
    pub scroll: usize,
    /// The wheel scrolled the list: it stays there, even with the cursor off
    /// screen, until the cursor moves.
    pub detached: bool,
    pub filter: TextInput,
    /// The filter input has the keyboard (`explorer.filter`).
    pub filtering: bool,
    /// Screen area of the rows (mouse), kept by the renderer.
    pub area: Rect,
    /// The user hid the explorer (the panes take the whole width); focusing it shows it again.
    pub hidden: bool,
    /// The width the user chose, in columns (`None`: [`App::explorer_width`]'s default); kept
    /// as chosen, drawn within the limits of the terminal's width.
    pub width: Option<u16>,
}

impl Default for Explorer {
    fn default() -> Self {
        Self {
            cursor: RowKind::NewConnection,
            last: 0,
            scroll: 0,
            detached: false,
            filter: TextInput::default(),
            filtering: false,
            area: Rect::default(),
            hidden: false,
            width: None,
        }
    }
}

impl Explorer {
    /// Index of the cursor in `rows`: its row, else where it was (clamped).
    pub fn index(&self, rows: &[Row]) -> usize {
        rows.iter().position(|r| r.kind == self.cursor).unwrap_or(self.last.min(rows.len().saturating_sub(1)))
    }

    /// Put the cursor on row `i`.
    pub fn select(&mut self, rows: &[Row], i: usize) {
        if let Some(r) = rows.get(i) {
            self.cursor = r.kind.clone();
            self.last = i;
        }
        self.detached = false;
    }

    /// Put the cursor on the row of `kind` (it takes effect even before the row exists).
    pub fn select_kind(&mut self, kind: RowKind) {
        self.cursor = kind;
        self.detached = false;
    }

    /// The row the cursor is on.
    pub fn cursor(&self) -> &RowKind {
        &self.cursor
    }
}

/// The `/` filter's text, folded once: it matches a name that has it as a part, ignoring case
/// (as `name.to_lowercase().contains(..)` would), without allocating for each name.
pub struct Needle {
    /// The trimmed text, lowercased.
    chars: Vec<char>,
    /// The same as bytes when it is all ASCII.
    ascii: Option<Vec<u8>>,
}

impl Needle {
    pub fn new(query: &str) -> Self {
        let folded = query.trim().to_lowercase();
        let ascii = folded.is_ascii().then(|| folded.as_bytes().to_vec());
        Self { chars: folded.chars().collect(), ascii }
    }

    /// No text: everything passes.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Whether `text` has the filter's text as a part, ignoring case.
    pub fn matches(&self, text: &str) -> bool {
        if self.chars.is_empty() {
            return true;
        }
        if text.is_ascii() {
            // ASCII text lowercases to ASCII: only an ASCII filter can be in it.
            let Some(n) = &self.ascii else { return false };
            return text
                .as_bytes()
                .windows(n.len())
                .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b));
        }
        // Folded a character at a time: unlike `str::to_lowercase` for context-dependent cases
        // only (a final sigma folds to `σ`, not `ς`).
        text.char_indices().any(|(at, _)| {
            let mut folded = text[at..].chars().flat_map(char::to_lowercase);
            self.chars.iter().all(|c| folded.next() == Some(*c))
        })
    }

    /// Whether profile `p` passes: a part of its name, its host (`host:port`) or its database.
    fn profile(&self, p: &ConnectionConfig) -> bool {
        if self.matches(&p.name) {
            return true;
        }
        let (addr, db, _) = p.endpoint();
        self.matches(&addr) || self.matches(&db)
    }
}

/// How many columns `catalog` has for `schema.name` (`None`: it does not have the object).
fn column_count(catalog: &Catalog, schema: &str, name: &str) -> Option<usize> {
    catalog.relations.iter().find(|r| r.schema == schema && r.name == name).map(|r| r.columns.len())
}

impl App {
    /// The explorer's rows: "＋ New connection", then the "Connections" section (its head, then,
    /// open, folders before profiles at each level, each by name ignoring case). With a filter,
    /// what matches it and the way to it (see [`App::push_level`]), the head only when something
    /// under it matches; the sections below list their matching rows.
    pub fn explorer_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::new(RowKind::NewConnection, 0)];
        let needle = Needle::new(self.explorer.filter.text());
        let filtering = !needle.is_empty();
        // Open for the filter's matches (its own state stays as it is), as the other heads.
        rows.push(if filtering {
            Row::opened(RowKind::ConnectionsHeader, 0)
        } else {
            Row::new(RowKind::ConnectionsHeader, 0)
        });
        if self.connections_expanded || filtering {
            self.push_level(None, 1, filtering.then_some(&needle), &mut rows);
            if filtering && rows.len() == 2 {
                rows.truncate(1);
            }
        }
        self.push_tunnels(&mut rows);
        self.push_environments(&mut rows);
        self.push_scripts(&mut rows);
        rows
    }

    /// The "Environments" section: the head, then the environments in the file's order (with a
    /// filter, the ones whose name or badge matches), each with, opened, its profiles; without
    /// one, the row that adds the presets.
    fn push_environments(&self, rows: &mut Vec<Row>) {
        if !self.environments_shown() {
            return;
        }
        let q = self.explorer.filter.text().trim().to_lowercase();
        let filtering = !q.is_empty();
        let matches = |e: &datarig_core::policy::Environment| {
            [Some(&e.name), e.badge.as_ref()].into_iter().flatten().any(|t| t.to_lowercase().contains(&q))
        };
        let envs = self.environments();
        if filtering && !envs.iter().any(matches) {
            return;
        }
        rows.push(Row::new(RowKind::EnvironmentsHeader, 0));
        if !self.environments_expanded && !filtering {
            return;
        }
        if !filtering {
            rows.push(Row::new(RowKind::DefaultRules, 1));
            if self.default_rules_open {
                for c in self.default_rules_users() {
                    rows.push(Row::new(RowKind::DefaultRulesUser(c.id), 2));
                }
            }
        }
        if envs.is_empty() {
            rows.push(Row::new(RowKind::EnvironmentPresets, 1));
            return;
        }
        for e in envs.iter().filter(|e| !filtering || matches(e)) {
            rows.push(Row { hit: filtering, ..Row::new(RowKind::Environment(e.id), 1) });
            if self.environments_open.contains(&e.id) {
                for c in self.environment_users(&e.name) {
                    rows.push(Row::new(RowKind::EnvironmentUser(e.id, c.id), 2));
                }
            }
        }
    }

    /// The "Tunnels" section: the head, then the presets by name (with a filter, the ones whose
    /// name or bastion matches), each with why its connection was lost and, opened, the
    /// profiles connected through it now.
    fn push_tunnels(&self, rows: &mut Vec<Row>) {
        if !self.tunnels_shown() {
            return;
        }
        let q = self.explorer.filter.text().trim().to_lowercase();
        let filtering = !q.is_empty();
        let matches = |p: &datarig_core::profile::tunnel::TunnelPreset| {
            [p.name.clone(), App::bastion_text(&p.settings)].iter().any(|t| t.to_lowercase().contains(&q))
        };
        if filtering && !self.presets.iter().any(matches) {
            return;
        }
        rows.push(Row::new(RowKind::TunnelsHeader, 0));
        if !self.tunnels_expanded && !filtering {
            return;
        }
        if self.presets.is_empty() {
            rows.push(Row::new(RowKind::TunnelsEmpty, 1));
            return;
        }
        for p in self.presets.iter().filter(|p| !filtering || matches(p)) {
            rows.push(Row { hit: filtering, ..Row::new(RowKind::Tunnel(p.id), 1) });
            if self.preset_state(p.id) == super::tunnel::PresetState::Dead {
                rows.push(Row::new(RowKind::TunnelError(p.id), 2));
            }
            if self.tunnels_open.contains(&p.id) {
                for c in self.preset_connected_users(p.id) {
                    rows.push(Row::new(RowKind::TunnelUser(p.id, c.id), 2));
                }
            }
        }
    }

    /// The "Saved queries" section: the header, then the folders and scripts that are open
    /// (with a filter, the scripts whose path matches and their folders).
    fn push_scripts(&self, rows: &mut Vec<Row>) {
        use datarig_core::scripts::{is_within, name::fold, parent};
        if !self.scripts_available() {
            return;
        }
        rows.push(Row::new(RowKind::ScriptsHeader, 0));
        let q = fold(self.explorer.filter.text().trim());
        let filtering = !q.is_empty();
        if !self.scripts_expanded && !filtering {
            return;
        }
        if self.script_list.is_empty() && !filtering {
            rows.push(Row::new(RowKind::ScriptsEmpty, 1));
            return;
        }
        let matches = |path: &str| fold(datarig_core::scripts::stem_path(path)).contains(&q);
        let open = |path: &str| {
            let mut dir = parent(path);
            while let Some(d) = dir {
                if !self.script_folders.contains(d) {
                    return false;
                }
                dir = parent(d);
            }
            true
        };
        for e in &self.script_list {
            let shown = if filtering {
                if e.folder {
                    self.script_list.iter().any(|x| !x.folder && is_within(&x.path, &e.path) && matches(&x.path))
                } else {
                    matches(&e.path)
                }
            } else {
                open(&e.path)
            };
            if shown {
                let kind =
                    if e.folder { RowKind::ScriptFolder(e.path.clone()) } else { RowKind::Script(e.path.clone()) };
                rows.push(Row { hit: filtering && !e.folder, ..Row::new(kind, e.depth() + 1) });
            }
        }
    }

    /// The saved-queries folder `path` could not be read (said, as the dialogs do).
    pub fn script_unreadable(&self, path: &str) -> bool {
        self.script_list.iter().any(|e| e.folder && e.unreadable && e.path == path)
    }

    fn filtering_on(&self) -> bool {
        !self.explorer.filter.text().trim().is_empty()
    }

    /// The folders and profiles under `parent` at `depth`, with what is open below them. With a
    /// filter (`q`): a folder whose name matches shows open with everything in it as without
    /// the filter; a profile whose name, host or database matches shows as without it; a
    /// connected profile also shows, opened, for a database, a schema, an object or an item of
    /// an object's structure that matches among what was read (see [`Tree::rows_matching`]);
    /// a folder shows, opened, for a match inside it. Nothing else is listed, and no expansion
    /// changes.
    fn push_level(&self, parent: Option<&FolderPath>, depth: usize, q: Option<&Needle>, rows: &mut Vec<Row>) {
        for f in self.folders.children(parent) {
            match q {
                None => {
                    rows.push(Row::new(RowKind::Folder(f.clone()), depth));
                    if self.folders.is_expanded(f) {
                        self.push_level(Some(f), depth + 1, None, rows);
                    }
                }
                Some(n) if n.matches(f.name()) => {
                    rows.push(Row { opened: true, ..Row::hit(RowKind::Folder(f.clone()), depth) });
                    self.push_level(Some(f), depth + 1, None, rows);
                }
                Some(n) => {
                    let at = rows.len();
                    rows.push(Row::opened(RowKind::Folder(f.clone()), depth));
                    self.push_level(Some(f), depth + 1, Some(n), rows);
                    if rows.len() == at + 1 {
                        rows.truncate(at);
                    }
                }
            }
        }
        let mut here: Vec<&ConnectionConfig> = self
            .profiles
            .iter()
            .filter(|p| {
                // A folder the folder list does not know shows its profiles at the top.
                let folder = p.folder_path().filter(|f| self.folders.contains(f));
                folder.as_ref() == parent
            })
            .collect();
        here.sort_by_key(|p| p.name.to_lowercase());
        for p in here {
            let c = self.conns.get(p.id);
            match q {
                Some(n) if !n.profile(p) => {
                    if let Some(c) = c.filter(|c| c.connected) {
                        let at = rows.len();
                        rows.push(Row::opened(RowKind::Profile(p.id), depth));
                        self.push_databases_matching(p.id, c, depth + 1, n, rows);
                        if rows.len() == at + 1 {
                            rows.truncate(at);
                        }
                    }
                    continue;
                }
                Some(_) => rows.push(Row::hit(RowKind::Profile(p.id), depth)),
                None => rows.push(Row::new(RowKind::Profile(p.id), depth)),
            }
            let Some(c) = c else { continue };
            if c.state() == NodeState::Failed {
                rows.push(Row::new(RowKind::ProfileError(p.id), depth + 1));
            }
            if c.connected && c.expanded {
                self.push_databases(p.id, c, depth + 1, rows);
            }
        }
    }

    /// The databases of connected profile `id` at `depth` (DataGrip's level): its
    /// own first (its schema tree from the profile's metadata session), then the others the
    /// server lists, each opening to its schema tree through that database's aux session.
    fn push_databases(&self, id: ProfileId, c: &ProfileConn, depth: usize, rows: &mut Vec<Row>) {
        let structured = self.structure_on(id);
        // One level (MySQL: a database is the schema): the schemas right under the profile.
        if self.schema_only(Some(id)) {
            for r in c.tree.rows_with(|s, n| column_count(&c.catalog, s, n), structured) {
                rows.push(Row::new(RowKind::Node(id, r.node), depth + r.depth));
            }
            return;
        }
        rows.push(Row::new(RowKind::Database(id, None), depth));
        if c.own_open {
            // An open table shows its structure, or its columns from the completion catalog.
            for r in c.tree.rows_with(|s, n| column_count(&c.catalog, s, n), structured) {
                rows.push(Row::new(RowKind::Node(id, r.node), depth + 1 + r.depth));
            }
        }
        let own = self.own_database(id);
        let others = match &c.databases {
            Some(Ok(list)) => list.iter().filter(|d| **d != own).collect::<Vec<_>>(),
            Some(Err(_)) => return rows.push(Row::new(RowKind::DatabasesNote(id), depth)),
            None if c.databases_asked => return rows.push(Row::new(RowKind::DatabasesNote(id), depth)),
            None => Vec::new(),
        };
        for db in others {
            rows.push(Row::new(RowKind::Database(id, Some(db.clone())), depth));
            if c.open_dbs.contains(db) {
                self.push_other_database(id, db, depth + 1, rows);
            }
        }
    }

    /// What open database `db` of profile `id` (not its own) shows at `depth`: its schema tree,
    /// or that it is opening, or why it cannot be read.
    fn push_other_database(&self, id: ProfileId, db: &str, depth: usize, rows: &mut Vec<Row>) {
        match self.conns.aux(id, db) {
            // Opening (the profile connected: it opens on the next use).
            None => rows.push(Row::new(RowKind::AuxNode(id, db.to_string(), Node::Loading(None)), depth)),
            Some(a) if a.tree.schemas.is_empty() && matches!(a.schemas, Some(Err(_))) => {
                rows.push(Row::new(RowKind::DatabaseNote(id, db.to_string()), depth));
            }
            Some(a) => {
                let structured = self.structure_on(id);
                for r in a.tree.rows_with(|s, n| column_count(&a.catalog, s, n), structured) {
                    rows.push(Row::new(RowKind::AuxNode(id, db.to_string(), r.node), depth + r.depth));
                }
            }
        }
    }

    /// What the filter `q` shows of connected profile `id`, whose own name, host and database do
    /// not match, at `depth`: a database whose name matches as without the filter, else,
    /// opened, the database (or, with one level, the profile) with the matches of its schema
    /// tree. Only trees already read are searched.
    fn push_databases_matching(&self, id: ProfileId, c: &ProfileConn, depth: usize, q: &Needle, rows: &mut Vec<Row>) {
        let structured = self.structure_on(id);
        // The matches of a schema tree, each node a row of `kind` at `depth` and below.
        let matches =
            |tree: &Tree, catalog: &Catalog, depth: usize, kind: &dyn Fn(Node) -> RowKind, rows: &mut Vec<Row>| {
                for r in tree.rows_matching(|s, n| column_count(catalog, s, n), structured, |t| q.matches(t)) {
                    rows.push(Row { opened: r.opened, hit: r.hit, ..Row::new(kind(r.node), depth + r.depth) });
                }
            };
        if self.schema_only(Some(id)) {
            return matches(&c.tree, &c.catalog, depth, &|n| RowKind::Node(id, n), rows);
        }
        let own = self.own_database(id);
        if q.matches(&own) {
            rows.push(Row::hit(RowKind::Database(id, None), depth));
            if c.own_open {
                for r in c.tree.rows_with(|s, n| column_count(&c.catalog, s, n), structured) {
                    rows.push(Row::new(RowKind::Node(id, r.node), depth + 1 + r.depth));
                }
            }
        } else {
            let at = rows.len();
            rows.push(Row::opened(RowKind::Database(id, None), depth));
            matches(&c.tree, &c.catalog, depth + 1, &|n| RowKind::Node(id, n), rows);
            if rows.len() == at + 1 {
                rows.truncate(at);
            }
        }
        let Some(Ok(list)) = &c.databases else { return };
        for db in list.iter().filter(|d| **d != own) {
            if q.matches(db) {
                rows.push(Row::hit(RowKind::Database(id, Some(db.clone())), depth));
                if c.open_dbs.contains(db) {
                    self.push_other_database(id, db, depth + 1, rows);
                }
                continue;
            }
            let Some(a) = self.conns.aux(id, db) else { continue };
            let at = rows.len();
            rows.push(Row::opened(RowKind::Database(id, Some(db.clone())), depth));
            matches(&a.tree, &a.catalog, depth + 1, &|n| RowKind::AuxNode(id, db.clone(), n), rows);
            if rows.len() == at + 1 {
                rows.truncate(at);
            }
        }
    }

    /// Profile `id`'s driver reads a table's structure (`Capabilities::structure`): an open
    /// table shows it instead of its catalog columns.
    pub(super) fn structure_on(&self, id: ProfileId) -> bool {
        self.profile(id).and_then(|p| self.driver(&p.driver)).is_some_and(|d| d.capabilities().structure)
    }

    /// Ask for the databases of profile `id`'s server for the explorer, once (again after a
    /// failure: `again`).
    pub(super) fn ask_databases(&mut self, id: ProfileId, again: bool) {
        // One level: the schemas are the databases.
        if self.schema_only(Some(id)) {
            return;
        }
        let c = self.conns.entry(id);
        if !c.connected || c.databases_asked || (matches!(c.databases, Some(Ok(_))) && !again) {
            return;
        }
        c.databases = None;
        c.databases_asked = true;
        self.send_meta(id, DbCommand::LoadDatabases);
    }

    /// The whole line of `row` as the explorer draws it (indentation, arrow, icons, marks and
    /// details), not cut at the explorer's width.
    pub fn explorer_line_text(&self, row: &Row) -> String {
        crate::widgets::explorer::row_text(self, row)
    }

    /// The row under the explorer's cursor.
    pub fn explorer_row(&self) -> Option<Row> {
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        rows.into_iter().nth(i)
    }

    /// Index of the explorer's cursor.
    pub fn explorer_selected(&self) -> usize {
        self.explorer.index(&self.explorer_rows())
    }

    /// The profile the explorer's cursor is on (its node, error line or schema tree).
    pub fn selected_profile(&self) -> Option<ProfileId> {
        self.explorer_row().and_then(|r| r.profile())
    }

    /// The folder a new profile goes to: the selected folder, else the selected profile's.
    pub(super) fn selected_folder(&self) -> Option<FolderPath> {
        match self.explorer_row()?.kind {
            RowKind::Folder(f) => Some(f),
            kind => {
                let id = Row::new(kind, 0).profile()?;
                self.profile(id)?.folder_path().filter(|f| self.folders.contains(f))
            }
        }
    }

    /// Put the explorer's cursor on profile `id`, opening the folders around it.
    pub(super) fn reveal_profile(&mut self, id: ProfileId) {
        if !self.connections_expanded {
            self.connections_expanded = true;
            self.mark_workspace();
        }
        if let Some(f) = self.profile(id).and_then(ConnectionConfig::folder_path) {
            for a in f.with_ancestors() {
                if !self.folders.is_expanded(&a) {
                    self.folders.toggle(&a);
                }
            }
        }
        self.explorer.select_kind(RowKind::Profile(id));
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        self.explorer.select(&rows, i);
    }

    fn explorer_move(&mut self, to: impl FnOnce(usize, usize) -> usize) {
        let rows = self.explorer_rows();
        let i = to(self.explorer.index(&rows), rows.len());
        self.explorer.select(&rows, i.min(rows.len().saturating_sub(1)));
    }

    /// Put the cursor on the row above the current one (its nearest ancestor).
    fn explorer_parent(&mut self) {
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        let depth = rows[i].depth;
        if let Some(p) = rows[..i].iter().rposition(|r| r.depth < depth) {
            self.explorer.select(&rows, p);
        }
    }

    /// Whether the row can be expanded: `Some(open)`.
    fn row_expanded(&self, row: &Row) -> Option<bool> {
        match &row.kind {
            RowKind::Folder(f) => Some(self.folders.is_expanded(f)),
            RowKind::Profile(id) => Some(self.conns.get(*id).is_some_and(|c| c.connected && c.expanded)),
            RowKind::Database(id, None) => self.conns.get(*id).map(|c| c.own_open),
            RowKind::Database(id, Some(db)) => self.conns.get(*id).map(|c| c.open_dbs.contains(db)),
            RowKind::Node(id, n) => self.conns.get(*id).and_then(|c| c.tree.is_expanded(*n)),
            RowKind::AuxNode(id, db, n) => self.conns.aux(*id, db).and_then(|a| a.tree.is_expanded(*n)),
            RowKind::ConnectionsHeader => Some(self.connections_expanded || self.filtering_on()),
            RowKind::ScriptsHeader => Some(self.scripts_expanded || self.filtering_on()),
            // A folder that cannot be read has nothing to open (never shown as an empty one).
            RowKind::ScriptFolder(p) if self.script_unreadable(p) => None,
            RowKind::ScriptFolder(p) => Some(self.script_folders.contains(p) || self.filtering_on()),
            RowKind::TunnelsHeader => Some(self.tunnels_expanded || self.filtering_on()),
            // Only one some profile is connected through has something to open.
            RowKind::Tunnel(id) => {
                (!self.preset_connected_users(*id).is_empty()).then(|| self.tunnels_open.contains(id))
            }
            RowKind::EnvironmentsHeader => Some(self.environments_expanded || self.filtering_on()),
            RowKind::DefaultRules => (!self.default_rules_users().is_empty()).then_some(self.default_rules_open),
            RowKind::Environment(id) => self
                .environment(*id)
                .filter(|e| !self.environment_users(&e.name).is_empty())
                .map(|_| self.environments_open.contains(id)),
            _ => None,
        }
    }

    /// Whether the renderer draws an arrow on `row` and which one: open on a row the filter
    /// opened.
    pub fn explorer_arrow(&self, row: &Row) -> Option<bool> {
        if row.opened { Some(true) } else { self.row_expanded(row) }
    }

    /// Expand (`Some(true)`), collapse (`Some(false)`) or toggle (`None`) the row. A profile that
    /// is not connected connects (and expands once it is). A row the filter shows open keeps
    /// its own expansion: a change would not show (and could ask the server for something).
    fn explorer_set_open(&mut self, row: &Row, want: Option<bool>) {
        if row.opened {
            return;
        }
        match &row.kind {
            RowKind::Folder(f) => {
                let open = self.folders.is_expanded(f);
                if want.unwrap_or(!open) != open {
                    self.folders.toggle(f);
                    self.mark_workspace();
                }
            }
            RowKind::Profile(id) | RowKind::ProfileError(id) => {
                let id = *id;
                match self.conns.state(id) {
                    NodeState::Connected => {
                        if let Some(c) = self.conns.get_mut(id) {
                            c.expanded = want.unwrap_or(!c.expanded);
                            if c.expanded {
                                self.ask_databases(id, false);
                            }
                        }
                    }
                    NodeState::Connecting => {}
                    _ if want != Some(false) => self.connect_from_explorer(id),
                    _ => {}
                }
            }
            RowKind::Database(id, None) => {
                if let Some(c) = self.conns.get_mut(*id) {
                    c.own_open = want.unwrap_or(!c.own_open);
                }
            }
            RowKind::Database(id, Some(db)) => {
                let (id, db) = (*id, db.clone());
                let Some(c) = self.conns.get_mut(id) else { return };
                let open = want.unwrap_or(!c.open_dbs.contains(&db));
                if open {
                    c.open_dbs.insert(db.clone());
                    self.ensure_aux(id, &db);
                } else {
                    c.open_dbs.remove(&db);
                }
            }
            // Asked again (the list, or the database whose session failed).
            RowKind::DatabasesNote(id) => {
                if matches!(self.conns.get(*id).map(|c| &c.databases), Some(Some(Err(_)))) {
                    self.ask_databases(*id, true);
                }
            }
            RowKind::DatabaseNote(id, db) => {
                let (id, db) = (*id, db.clone());
                self.ensure_aux(id, &db);
            }
            RowKind::Node(id, n) => {
                let id = *id;
                // A real driver without a table's structure yet (MySQL) shows its columns only:
                // said each time a table opens, so the missing keys and indexes do not read as none.
                let opens =
                    matches!(n, Node::Object(..)) && self.row_expanded(row) == Some(false) && want != Some(false);
                if opens && self.schema_only(Some(id)) && !self.structure_on(id) {
                    self.flash(Notice::new(Label::TreeColumnsOnly, Level::Info));
                }
                let Some(c) = self.conns.get_mut(id) else { return };
                let a = c.tree.set_expanded(*n, want);
                self.tree_action(id, a);
            }
            RowKind::AuxNode(id, db, n) => {
                let (id, db) = (*id, db.clone());
                let Some(a) = self.conns.aux_mut(id, &db) else { return };
                let action = a.tree.set_expanded(*n, want);
                self.aux_tree_action(id, &db, action);
            }
            RowKind::ConnectionsHeader => {
                self.connections_expanded = want.unwrap_or(!self.connections_expanded);
                self.mark_workspace();
            }
            RowKind::ScriptsHeader => {
                self.scripts_expanded = want.unwrap_or(!self.scripts_expanded);
                self.mark_workspace();
            }
            RowKind::ScriptFolder(p) if self.script_unreadable(p) => {}
            RowKind::ScriptFolder(p) => {
                let open = self.script_folders.contains(p);
                if want.unwrap_or(!open) != open {
                    if open {
                        self.script_folders.remove(p);
                    } else {
                        self.script_folders.insert(p.clone());
                    }
                    self.mark_workspace();
                }
            }
            RowKind::TunnelsHeader => self.tunnels_expanded = want.unwrap_or(!self.tunnels_expanded),
            RowKind::Tunnel(id) => {
                let open = self.tunnels_open.contains(id);
                if self.row_expanded(row).is_some() && want.unwrap_or(!open) != open {
                    if open {
                        self.tunnels_open.remove(id);
                    } else {
                        self.tunnels_open.insert(*id);
                    }
                }
            }
            RowKind::EnvironmentsHeader => {
                self.environments_expanded = want.unwrap_or(!self.environments_expanded);
            }
            RowKind::DefaultRules => {
                if self.row_expanded(row).is_some() {
                    self.default_rules_open = want.unwrap_or(!self.default_rules_open);
                }
            }
            RowKind::Environment(id) => {
                let open = self.environments_open.contains(id);
                if self.row_expanded(row).is_some() && want.unwrap_or(!open) != open {
                    if open {
                        self.environments_open.remove(id);
                    } else {
                        self.environments_open.insert(*id);
                    }
                }
            }
            RowKind::NewConnection
            | RowKind::Script(_)
            | RowKind::ScriptsEmpty
            | RowKind::TunnelError(_)
            | RowKind::TunnelUser(..)
            | RowKind::TunnelsEmpty
            | RowKind::EnvironmentUser(..)
            | RowKind::DefaultRulesUser(_)
            | RowKind::EnvironmentPresets => {}
        }
    }

    /// Enter / `l` / double-click on a profile that is not connected: connect, expand it once
    /// connected, and open a console tab if it has none (the focus stays here).
    pub(super) fn connect_from_explorer(&mut self, id: ProfileId) {
        if self.profile(id).is_none() {
            return;
        }
        let c = self.conns.entry(id);
        c.expand_on_connect = true;
        c.console = Some(false);
        self.connect(id);
    }

    pub(super) fn explorer_action(&mut self, e: ExplorerAction) {
        let Some(row) = self.explorer_row() else { return };
        match e {
            ExplorerAction::Down => self.explorer_move(|i, _| i + 1),
            ExplorerAction::Up => self.explorer_move(|i, _| i.saturating_sub(1)),
            ExplorerAction::Top => self.explorer_move(|_, _| 0),
            ExplorerAction::Bottom => self.explorer_move(|_, n| n.saturating_sub(1)),
            ExplorerAction::Expand => self.explorer_set_open(&row, Some(true)),
            ExplorerAction::Collapse => {
                if !row.opened && self.row_expanded(&row) == Some(true) {
                    self.explorer_set_open(&row, Some(false));
                } else {
                    self.explorer_parent();
                }
            }
            ExplorerAction::Activate => self.explorer_activate(&row),
            ExplorerAction::Refresh => {
                if matches!(row.kind, RowKind::ScriptsHeader | RowKind::ScriptFolder(_) | RowKind::Script(_)) {
                    return self.refresh_scripts();
                }
                let Some(id) = row.profile() else { return };
                if !self.conns.is_connected(id) {
                    return;
                }
                // Another database: its aux session reads its tree (or a node) again.
                if let RowKind::Database(_, Some(db)) | RowKind::AuxNode(_, db, _) | RowKind::DatabaseNote(_, db) =
                    &row.kind
                {
                    let node = match row.kind {
                        RowKind::AuxNode(_, _, n) => Some(n),
                        _ => None,
                    };
                    let db = db.clone();
                    self.ensure_aux(id, &db);
                    let structured = self.structure_on(id);
                    let Some(a) = self.conns.aux_mut(id, &db) else { return };
                    let action = a.tree.refresh(node, structured);
                    self.aux_tree_action(id, &db, action);
                    return;
                }
                let node = match row.kind {
                    RowKind::Node(_, n) => Some(n),
                    _ => None,
                };
                // The profile's node reads the server's databases again too.
                if matches!(row.kind, RowKind::Profile(_) | RowKind::DatabasesNote(_)) {
                    self.ask_databases(id, true);
                }
                let structured = self.structure_on(id);
                let Some(c) = self.conns.get_mut(id) else { return };
                c.expanded = true;
                c.own_open |= matches!(row.kind, RowKind::Database(..));
                let a = c.tree.refresh(node, structured);
                self.tree_action(id, a);
            }
            ExplorerAction::Filter => self.explorer.filtering = true,
            ExplorerAction::FilterClear => {
                self.explorer.filtering = false;
                self.explorer_clear_filter();
            }
            ExplorerAction::FilterAccept => self.explorer.filtering = false,
            ExplorerAction::Back => {
                let attempt = row.profile().filter(|id| self.conns.state(*id) == NodeState::Connecting);
                if self.cancel_test() {
                    self.report_test();
                } else if let Some(id) = attempt.or_else(|| self.latest_attempt()) {
                    self.cancel_connect(id);
                } else if !self.explorer.filter.text().is_empty() {
                    self.explorer_clear_filter();
                }
            }
            ExplorerAction::ContextMenu => self.open_context_menu_here(),
            ExplorerAction::ConsoleHere => {
                self.console_here(&row);
            }
            ExplorerAction::Move => self.open_move(),
            ExplorerAction::NewFolder => self.open_new_folder(),
            ExplorerAction::Rename => self.open_rename(),
            ExplorerAction::ShowDdl => self.explorer_show_ddl(false),
            ExplorerAction::ShowFunctionDdl => self.explorer_show_ddl(true),
            ExplorerAction::Delete => match row.kind {
                RowKind::Profile(id) => self.request_delete_profile(id),
                RowKind::Tunnel(id) | RowKind::TunnelError(id) => self.request_delete_tunnel(id),
                RowKind::Environment(id) => self.request_delete_environment(id),
                RowKind::DefaultRules => self.default_rules_fixed(),
                RowKind::Folder(f) => self.request_delete_folder(f),
                RowKind::Script(p) => self.request_delete_script(p),
                RowKind::ScriptFolder(p) => self.request_delete_script_folder(p),
                _ => {}
            },
        }
    }

    /// `O`, the menu's console item and a new console from the explorer (`Ctrl+T`): a new
    /// console on the row's profile, in the database and schema of the row (a database node is
    /// that database with its default schema; a schema, or anything under one, that database and
    /// schema), else with the profile's defaults. The editor gets the focus; the profile
    /// connects when it is not. `false`: the row has no profile.
    pub(super) fn console_here(&mut self, row: &Row) -> bool {
        let Some((id, context)) = self.console_here_context(row) else { return false };
        self.console_in(id, context);
        true
    }

    /// The profile and context of `O` on `row`: see [`App::console_here`].
    pub(super) fn console_here_context(&self, row: &Row) -> Option<(ProfileId, SessionContext)> {
        let id = row.profile()?;
        let (db, schema) = match &row.kind {
            RowKind::Node(_, n) => (None, self.conns.get(id).and_then(|c| c.tree.schema_of(*n)).map(str::to_string)),
            RowKind::Database(_, db) => (db.clone(), None),
            RowKind::DatabaseNote(_, db) => (Some(db.clone()), None),
            RowKind::AuxNode(_, db, n) => {
                let schema = self.conns.aux(id, db).and_then(|a| a.tree.schema_of(*n)).map(str::to_string);
                (Some(db.clone()), schema)
            }
            _ => (None, None),
        };
        Some((id, self.session_context(id, db, schema)))
    }

    /// What a node of the schema tree of profile `id`'s database `db` (not its own) asked for:
    /// its aux session reads it (opened again when it closed), and a table opens in a table tab
    /// bound to that database.
    pub(super) fn aux_tree_action(&mut self, id: ProfileId, db: &str, action: TreeAction) {
        let cmds = match action {
            TreeAction::None => return,
            TreeAction::LoadObjects(schema) => vec![DbCommand::LoadObjects { schema }],
            TreeAction::LoadStructure { schema, name } => {
                if !self.structure_on(id) {
                    return;
                }
                if let Some(a) = self.conns.aux_mut(id, db) {
                    a.tree.structure_loading(&schema, &name);
                }
                vec![DbCommand::LoadStructure { schema, table: name }]
            }
            TreeAction::Reveal { schema, name } => {
                let Some(a) = self.conns.aux_mut(id, db) else { return };
                let db = db.to_string();
                return match a.tree.reveal(&schema, &name) {
                    Reveal::Found(n) => self.explorer.select_kind(RowKind::AuxNode(id, db, n)),
                    Reveal::Pending(action) => self.aux_tree_action(id, &db, action),
                    Reveal::Missing => self.reveal_missing(&schema, &name),
                };
            }
            TreeAction::LoadSchemas => {
                if let Some(a) = self.conns.aux_mut(id, db) {
                    a.keys = Keys::Unknown;
                }
                vec![DbCommand::LoadSchemas, DbCommand::LoadCatalog, DbCommand::LoadKeys]
            }
            TreeAction::Open { schema, name } => return self.open_table(id, Some(db.to_string()), &schema, &name),
        };
        self.ensure_aux(id, db);
        if let Some(s) = self.conns.aux(id, db).and_then(|a| a.session.as_ref()) {
            for c in cmds {
                s.send(c);
            }
        }
    }

    /// A foreign key's table is not in the tree (a schema the explorer does not list, or a
    /// table dropped since): said.
    pub(super) fn reveal_missing(&mut self, schema: &str, name: &str) {
        let name = format!("{schema}.{name}");
        self.flash(Notice::new(Msg::TreeRevealMissing { name }, Level::Warning));
    }

    /// The objects of `schema` came for profile `id`'s tree (`db`: another database's): the
    /// cursor goes to the table that waited for them, if one did.
    pub(super) fn revealed(&mut self, id: ProfileId, db: Option<&str>, schema: &str) {
        let tree = match db {
            None => self.conns.get_mut(id).map(|c| &mut c.tree),
            Some(d) => self.conns.aux_mut(id, d).map(|a| &mut a.tree),
        };
        let Some((name, found)) = tree.and_then(|t| t.take_reveal(schema)) else { return };
        match (found, db) {
            (Some(n), None) => self.explorer.select_kind(RowKind::Node(id, n)),
            (Some(n), Some(d)) => self.explorer.select_kind(RowKind::AuxNode(id, d.to_string(), n)),
            (None, _) => self.reveal_missing(schema, &name),
        }
    }

    /// Clear the `/` filter. A cursor on a row the tree does not show without it goes to the
    /// nearest row above it (its parent, else the parent's) that it shows.
    fn explorer_clear_filter(&mut self) {
        let filtered = self.explorer_rows();
        self.explorer.filter.set("");
        let rows = self.explorer_rows();
        let Some(i) = filtered.iter().position(|r| &r.kind == self.explorer.cursor()) else { return };
        if rows.iter().any(|r| &r.kind == self.explorer.cursor()) {
            return;
        }
        let mut depth = filtered[i].depth;
        for r in filtered[..i].iter().rev() {
            if r.depth >= depth {
                continue;
            }
            if let Some(j) = rows.iter().position(|x| x.kind == r.kind) {
                return self.explorer.select(&rows, j);
            }
            depth = r.depth;
        }
    }

    /// Enter (or a double click) on `row`. On a row the filter shows open, only what opens
    /// something else (a table) runs: its own expansion does not change.
    fn explorer_activate(&mut self, row: &Row) {
        if row.opened
            && !matches!(row.kind, RowKind::Node(_, Node::Object(..)) | RowKind::AuxNode(_, _, Node::Object(..)))
        {
            return;
        }
        match &row.kind {
            RowKind::NewConnection => self.dispatch(Action::NewProfile),
            RowKind::Script(p) => {
                let p = p.clone();
                self.open_script(&p);
            }
            RowKind::ScriptsEmpty | RowKind::TunnelsEmpty | RowKind::TunnelError(_) => {}
            // The profile's own row, in the connections above.
            RowKind::TunnelUser(_, id) | RowKind::EnvironmentUser(_, id) | RowKind::DefaultRulesUser(id) => {
                let id = *id;
                self.reveal_profile(id);
            }
            RowKind::EnvironmentPresets => self.add_environment_presets(),
            RowKind::Node(id, n) => {
                let id = *id;
                let Some(c) = self.conns.get_mut(id) else { return };
                let a = c.tree.activate(*n);
                self.tree_action(id, a);
            }
            RowKind::AuxNode(id, db, n) => {
                let (id, db) = (*id, db.clone());
                let Some(a) = self.conns.aux_mut(id, &db) else { return };
                let action = a.tree.activate(*n);
                self.aux_tree_action(id, &db, action);
            }
            _ => self.explorer_set_open(row, None),
        }
    }

    /// The profile a profile action works on: the explorer's selection when it has the focus,
    /// else the active tab's profile.
    pub(super) fn action_profile(&self) -> Option<ProfileId> {
        if self.focus == Focus::Tree { self.selected_profile() } else { self.tab().profile }
    }

    /// The latest attempt in progress.
    pub(super) fn latest_attempt(&self) -> Option<ProfileId> {
        self.conns.connecting().max_by_key(|(_, c)| c.started).map(|(id, _)| id)
    }

    /// Mouse on the explorer: a click selects, a double click activates, the wheel scrolls.
    pub(super) fn explorer_mouse(&mut self, m: MouseEvent) {
        let area = self.explorer.area;
        match m.kind {
            // The wheel scrolls the list, not the cursor; the renderer clamps it.
            MouseEventKind::ScrollDown => {
                self.explorer.scroll += WHEEL_STEP as usize;
                self.explorer.detached = true;
            }
            MouseEventKind::ScrollUp => {
                self.explorer.scroll = self.explorer.scroll.saturating_sub(WHEEL_STEP as usize);
                self.explorer.detached = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let (x, y) = (m.column, m.row);
                if !area.contains(ratatui::layout::Position::new(x, y)) {
                    return;
                }
                let rows = self.explorer_rows();
                // The first row is pinned; the others scroll below it.
                let i = if y == area.y { 0 } else { 1 + self.explorer.scroll + usize::from(y - area.y - 1) };
                if i >= rows.len() {
                    return;
                }
                let double = self.explorer.index(&rows) == i
                    && self.last_click.is_some_and(|(t, _, ly)| t.elapsed() < DOUBLE_CLICK && ly == y);
                self.explorer.select(&rows, i);
                self.last_click = Some((Instant::now(), x, y));
                if double {
                    self.last_click = None;
                    self.explorer_activate(&rows[i]);
                }
            }
            _ => {}
        }
    }

    /// Typing in the explorer's `/` filter: the cursor moves to the first row that matches (of
    /// any kind).
    pub(super) fn explorer_filter_key(&mut self, key: KeyEvent) {
        if self.explorer.filter.handle_key(&key) == crate::widgets::text_input::InputResult::Changed {
            let rows = self.explorer_rows();
            let first = rows.iter().position(|r| r.hit).unwrap_or(0);
            let keep = rows.iter().any(|r| &r.kind == self.explorer.cursor());
            if !keep || self.filtering_on() {
                self.explorer.select(&rows, first);
            }
        }
    }
}

#[cfg(test)]
mod tests;
