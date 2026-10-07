//! What survives a restart: autosave of
//! every tab's text, the workspace state (`workspace.toml`: tabs, cursor, folders), restoring
//! it on launch without connecting anything, the lazy connect once a restored tab gets the
//! focus, and the instance lock that makes a second datarig workspace-read-only.
//!
//! Autosave writes a tab's text at most [`AUTOSAVE`] after an edit, and at once when the tab
//! is left, closed or the app quits. Consoles go to `<state>/consoles/<id>.sql`, saved queries
//! to their file (with the stamp check of [`ScriptStore::save`]); every write is atomic.

use super::*;
use datarig_core::fault::{ErrorLog, Fault, FaultKind, KeychainFault};
use datarig_core::i18n::io_reason;
use datarig_core::scripts::{SaveError, ScriptStore};
use datarig_core::workspace::{self, Acquired, ExplorerState, TabState, WorkspaceState};

/// Nobody wrote `text`: it is blank.
pub(super) fn unwritten(text: &str) -> bool {
    text.trim().is_empty()
}

/// What a save of a tab did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Saved {
    /// Written, or nothing to write.
    Ok,
    /// The script changed or went away on disk: nothing was written (the tab is in conflict).
    Conflict,
    /// The write failed.
    Failed(String),
}

/// What went wrong with console files while restoring the workspace.
#[derive(Default)]
struct ConsoleProblems {
    /// Files that are there but cannot be read (left in place).
    unreadable: u64,
    /// Files a tab of `workspace.toml` lists that are not there.
    missing: u64,
    /// The consoles folder itself cannot be read: why.
    folder: Option<String>,
}

impl App {
    /// The state directory, unless this instance may not write it.
    pub(super) fn state_dir(&self) -> Option<PathBuf> {
        if self.read_only { None } else { self.paths.state.clone() }
    }

    /// Launch: take the instance lock, open the saved queries and restore the workspace (a
    /// second instance restores nothing). Nothing connects.
    pub(super) fn open_workspace(&mut self) {
        if let Some(data) = self.paths.data.clone() {
            let store = ScriptStore::open(&data);
            if let Some(p) = &store.index_problem {
                let error = self.fault_text("scripts.index", &p.error);
                let msg = match &p.backup {
                    Some(b) => Msg::ScriptsIndexRebuilt { error, path: b.display().to_string(), count: p.kept as u64 },
                    None => Msg::ScriptsIndexKept { error },
                };
                self.notices.push(Notice::new(msg, Level::Warning));
            }
            self.scripts = Some(store);
            self.refresh_scripts();
        }
        let Some(state) = self.paths.state.clone() else { return };
        self.sweep_spill(&state);
        match workspace::acquire(&state) {
            Ok(Acquired::Owned(lock)) => self.lock = Some(lock),
            Ok(Acquired::Held { .. }) => {
                self.read_only = true;
                return;
            }
            Err(e) => {
                self.read_only = true;
                let error = self.io_text(&e);
                self.notices.push(Notice::new(Msg::WorkspaceLockFailed { error }, Level::Warning));
                return;
            }
        }
        self.restore_workspace(&state);
    }

    /// Rescan the saved queries (the explorer's section).
    pub(super) fn refresh_scripts(&mut self) {
        self.script_list = self.scripts.as_ref().map(ScriptStore::list).unwrap_or_default();
    }

    /// Bring back the tabs and folders of the last run. A tab of a saved query that is gone is
    /// dropped (and said so); a tab of a profile that is not among the profiles has no
    /// connection but keeps its binding (`Doc::kept_profile`): the config
    /// file may just be unreadable right now, so an unknown profile is never taken for a
    /// deleted one.
    ///
    /// Console files are never moved or deleted here. Every console file no restored tab
    /// refers to comes back as a "recovered" tab, whatever state `workspace.toml` was in: the
    /// only way a console leaves the workspace is the user closing its tab, which puts it in
    /// the trash, so such a file is a crash, a bug or damage, never garbage. A file that cannot
    /// be read is left where it is and tried again at the next launch. The trash is trimmed
    /// here, before anything new goes into it ([`workspace::trim_trash`]).
    fn restore_workspace(&mut self, state: &Path) {
        let loaded = workspace::load(state);
        if let Some((fault, bak)) = &loaded.broken {
            let error = self.fault_text("workspace.broken", fault);
            let msg = match bak {
                Some(b) => Msg::WorkspaceBroken { error, path: b.display().to_string() },
                None => Msg::WorkspaceBrokenKept { error },
            };
            self.notices.push(Notice::new(msg, Level::Warning));
        }
        if loaded.duplicates > 0 {
            let msg = Msg::WorkspaceDuplicateTabs { count: loaded.duplicates };
            self.notices.push(Notice::new(msg, Level::Warning));
        }
        // A later version's file: read, never written (neither is anything else of the
        // workspace), so nothing of it is lost; the banner says so. Read-only before any tab
        // comes back, so not even the first tab's save is tried.
        if let Some(version) = loaded.newer {
            self.workspace_newer = Some(version);
            self.read_only = true;
        }
        let ws = loaded.state;
        // Tabs of a later version: written back as they are (unknown is not absent).
        self.unknown_tabs = ws.unknown_tabs.clone();
        let expanded: Vec<FolderPath> =
            ws.explorer.expanded_folders.iter().filter_map(|f| FolderPath::parse(f).ok()).collect();
        for f in &expanded {
            if self.folders.contains(f) && !self.folders.is_expanded(f) {
                self.folders.toggle(f);
            }
        }
        if loaded.found {
            self.scripts_expanded = ws.explorer.scripts_expanded;
        }
        self.script_folders = ws.explorer.script_folders.iter().cloned().collect();
        self.explorer.hidden = ws.explorer.hidden;
        self.explorer.width = ws.explorer.width;
        let mut problems = ConsoleProblems::default();
        let mut restored: Vec<Tab> = Vec::new();
        let mut active = 0;
        for (i, t) in ws.tabs.iter().enumerate() {
            let Some(tab) = self.restored_tab(state, t, &mut problems) else { continue };
            if i <= ws.active {
                active = restored.len();
            }
            restored.push(tab);
        }
        let listed: Vec<String> =
            ws.tabs.iter().filter(|t| t.kind == workspace::TabKind::Console).map(|t| t.id.clone()).collect();
        restored.extend(self.recovered_consoles(state, &listed, &mut problems));
        self.console_notices(state, problems);
        if !restored.is_empty() {
            let n = restored.len();
            for t in restored {
                let id = self.tabs.insert_tab(usize::MAX, t);
                self.sync_tab_language(id);
            }
            self.tabs.activate(active.min(n - 1));
            self.entered_tab();
        }
        if loaded.newer.is_some() {
            return;
        }
        // Nothing just trashed can be deleted: this runs before any tab is closed.
        let _ = workspace::trim_trash(state, std::time::SystemTime::now());
        self.save_workspace();
    }

    /// Tabs for the console files that no tab of `workspace.toml` lists (`listed`). Files
    /// that cannot be read stay where they are (counted in `problems`).
    fn recovered_consoles(&mut self, state: &Path, listed: &[String], problems: &mut ConsoleProblems) -> Vec<Tab> {
        let mut out = Vec::new();
        let orphans = match workspace::orphan_consoles(state, listed) {
            Ok(o) => o,
            Err(e) => {
                problems.folder = Some(self.io_text(&e));
                Vec::new()
            }
        };
        for id in orphans {
            let Ok(Some(text)) = workspace::read_console(state, &id) else {
                problems.unreadable += 1;
                continue;
            };
            if unwritten(&text) {
                // Read, and nothing in it: no tab without a connection for it.
                let _ = workspace::remove_console(state, &id);
                continue;
            }
            let mut tab = Tab::new(TabId(0), TabKind::Console, None, Editor::new(&text));
            tab.doc.console_id = id;
            tab.doc.saved = text;
            tab.doc.written = true;
            tab.doc.recovered = true;
            out.push(tab);
        }
        if !out.is_empty() {
            let count = out.len() as u64;
            self.notices.push(Notice::new(Msg::WorkspaceConsolesRecovered { count }, Level::Warning));
        }
        out
    }

    /// Say what went wrong with console files at launch.
    fn console_notices(&mut self, state: &Path, p: ConsoleProblems) {
        let path = state.join(workspace::CONSOLES).display().to_string();
        if p.unreadable > 0 {
            let msg = Msg::WorkspaceConsolesUnreadable { count: p.unreadable, path: path.clone() };
            self.notices.push(Notice::new(msg, Level::Warning));
        }
        if p.missing > 0 {
            self.notices.push(Notice::new(Msg::WorkspaceConsolesMissing { count: p.missing }, Level::Warning));
        }
        if let Some(error) = p.folder {
            self.notices.push(Notice::new(Msg::WorkspaceConsolesFolderUnreadable { path, error }, Level::Warning));
        }
    }

    /// One tab of `workspace.toml`, or `None` when it cannot come back. A console whose file
    /// is there but cannot be read is left out (counted in `problems`): its file is kept and
    /// never written over with an empty text, and it is tried again at the next launch (no
    /// tab lists it then, so it comes back as a recovered one). A console whose file is
    /// missing comes back empty, and is said so. A console without any connection that nobody
    /// wrote in (read, and blank) does not come back: the workspace
    /// never starts with an unbound console; its file goes.
    fn restored_tab(&mut self, state: &Path, t: &TabState, problems: &mut ConsoleProblems) -> Option<Tab> {
        let profile = t.profile.filter(|p| self.profile(*p).is_some());
        let kept = t.profile.filter(|p| self.profile(*p).is_none());
        let unbound = t.profile.is_none();
        let pane = t.results.map_or_else(super::tabs::PaneLayout::default, |r| super::tabs::PaneLayout {
            share: super::tabs::PaneLayout::clamped(r.share),
            hidden: r.hidden,
            zoom: r.maximized.then_some(Focus::Results),
        });
        if let (workspace::TabKind::Table, Some((schema, name))) = (t.kind, &t.table) {
            // Its query, not run: nothing is sent until the user asks.
            let table = super::tabs::TableRef { schema: schema.clone(), name: name.clone() };
            let query = table.query();
            let mut tab = Tab::new(TabId(0), TabKind::Table, profile, Editor::new(&query));
            tab.doc.saved = query;
            tab.doc.written = true;
            tab.doc.table = Some(table);
            tab.doc.kept_profile = kept;
            tab.pane = pane;
            // Its database (another database's table); its schema is the default.
            tab.context = datarig_core::driver::SessionContext { database: t.database.clone(), schema: None };
            return Some(tab);
        }
        if let (workspace::TabKind::Ddl, Some(object)) = (t.kind, &t.ddl) {
            // Its object, not read: nothing is sent until the user asks.
            let mut tab = Tab::new(TabId(0), TabKind::Ddl, profile, Editor::read_only(""));
            tab.doc.ddl = Some(super::tabs::DdlTab::new(object.clone()));
            tab.doc.written = true;
            tab.doc.kept_profile = kept;
            tab.pane = pane;
            tab.context = datarig_core::driver::SessionContext { database: t.database.clone(), schema: None };
            return Some(tab);
        }
        let (text, stamp, script, written) = match (&t.kind, &t.script) {
            (workspace::TabKind::Script, Some(path)) => {
                let read = self.scripts.as_ref().map(|s| s.read(path));
                match read {
                    Some(Ok((text, stamp))) => (text, Some(stamp), Some(path.clone()), true),
                    _ => {
                        let name = path.clone();
                        self.notices.push(Notice::new(Msg::ScriptsRestoreMissing { name }, Level::Warning));
                        return None;
                    }
                }
            }
            _ => match workspace::read_console(state, &t.id) {
                Ok(Some(text)) if unbound && unwritten(&text) => {
                    let _ = workspace::remove_console(state, &t.id);
                    return None;
                }
                Ok(Some(text)) => (text, None, None, true),
                Ok(None) if unbound => {
                    problems.missing += 1;
                    return None;
                }
                Ok(None) => {
                    problems.missing += 1;
                    (String::new(), None, None, false)
                }
                Err(_) => {
                    problems.unreadable += 1;
                    return None;
                }
            },
        };
        let mut editor = Editor::new(&text);
        editor.row = t.cursor.0.min(editor.lines.len() - 1);
        editor.col = t.cursor.1;
        editor.top = t.top.min(editor.row);
        let kind = if script.is_some() { TabKind::Script } else { TabKind::Console };
        let mut tab = Tab::new(TabId(0), kind, profile, editor);
        // A saved query's id names no file: a new one never meets a console's.
        tab.doc.console_id = if script.is_some() { workspace::new_id() } else { t.id.clone() };
        tab.doc.script = script;
        tab.doc.saved = text;
        tab.doc.written = written;
        tab.doc.stamp = stamp;
        tab.doc.lazy_connect = profile.is_some();
        tab.doc.kept_profile = kept;
        tab.doc.console_no = if kind == TabKind::Console { t.console } else { 0 };
        tab.pane = pane;
        // A query tab's database and schema.
        tab.context = datarig_core::driver::SessionContext { database: t.database.clone(), schema: t.schema.clone() };
        Some(tab)
    }

    /// The workspace as `workspace.toml` keeps it.
    pub fn workspace_state(&self) -> WorkspaceState {
        let tabs = self
            .tabs
            .iter()
            .map(|t| TabState {
                id: t.doc.console_id.clone(),
                kind: match (&t.doc.table, &t.doc.script) {
                    _ if t.doc.ddl.is_some() => workspace::TabKind::Ddl,
                    (Some(_), _) => workspace::TabKind::Table,
                    (None, Some(_)) => workspace::TabKind::Script,
                    (None, None) => workspace::TabKind::Console,
                },
                script: t.doc.script.clone(),
                profile: t.saved_profile(),
                cursor: (t.editor.row, t.editor.col),
                top: t.editor.top,
                console: if t.kind == TabKind::Console { t.doc.console_no } else { 0 },
                table: t.doc.table.as_ref().map(|r| (r.schema.clone(), r.name.clone())),
                ddl: t.doc.ddl.as_ref().map(|d| d.object.clone()),
                results: Some(workspace::PaneState {
                    share: t.pane.share,
                    hidden: t.pane.hidden,
                    maximized: t.pane.zoom == Some(Focus::Results),
                }),
                database: t.context.database.clone(),
                schema: t.context.schema.clone(),
            })
            .collect();
        WorkspaceState {
            active: self.tabs.active_index(),
            explorer: ExplorerState {
                expanded_folders: self.folders.expanded().map(ToString::to_string).collect(),
                scripts_expanded: self.scripts_expanded,
                script_folders: self.script_folders.iter().cloned().collect(),
                hidden: self.explorer.hidden,
                width: self.explorer.width,
            },
            tabs,
            unknown_tabs: self.unknown_tabs.clone(),
        }
    }

    /// Write every tab that changed and the workspace state now (tab opened, closed or left;
    /// quitting). A second instance writes only its saved queries. Returns whether
    /// `workspace.toml` was written.
    pub(super) fn save_workspace(&mut self) -> bool {
        self.save_workspace_with(true)
    }

    /// [`App::save_workspace`]; `tabs: false` (the autosave timer) leaves the tabs' texts to
    /// their own autosave times, so a large text is not written on every cursor move.
    fn save_workspace_with(&mut self, tabs: bool) -> bool {
        self.workspace_due = None;
        let ids: Vec<TabId> = self.tabs.iter().map(|t| t.id).collect();
        for id in ids {
            let dirty = tabs && self.tabs.get(id).is_some_and(|t| t.dirty() && !t.doc.conflict);
            if dirty {
                self.save_tab(id);
            }
            self.sync_binding(id);
        }
        let Some(state) = self.state_dir() else { return false };
        if let Err(e) = workspace::save(&state, &self.workspace_state()) {
            let error = self.io_text(&e);
            self.flash(Notice::new(Msg::WorkspaceSaveFailed { error }, Level::Error));
            return false;
        }
        true
    }

    /// A reason (see [`io_reason`]) in the UI language, for a message's `{error}`.
    pub(super) fn reason_text(&self, reason: &Msg) -> String {
        self.i18n.msg(reason).to_string()
    }

    /// Why a file operation failed, friendly and in the UI language (the OS's text goes to
    /// the error log).
    pub(super) fn io_text(&self, e: &std::io::Error) -> String {
        self.fault_text("io", &Fault::io(e))
    }

    /// Why something failed, in the UI language: a short reason ([`fault_reason`]). The raw
    /// detail goes to `errors.log` in the state directory (with `context`, a message key), never
    /// to the screen.
    pub(super) fn fault_text(&self, context: &str, f: &Fault) -> String {
        ErrorLog::new(self.paths.errors_log()).record(context, f);
        self.reason_text(&fault_reason(f))
    }

    /// Why a save failed, as a reason (see [`io_reason`]).
    pub(super) fn save_reason(&self, e: &SaveError) -> Msg {
        match e {
            SaveError::Io(kind, _) => io_reason(*kind),
            SaveError::Exists(_) => Msg::Label(Label::IoExists),
            SaveError::Conflict => self.conflict_later(),
            SaveError::Missing => Msg::Label(Label::IoNotFound),
        }
    }

    /// Delete the spill files of runs that crashed (only theirs: see `results::spill::sweep`).
    /// What cannot be looked at or deleted goes to the error log; nothing is guessed.
    fn sweep_spill(&mut self, state: &std::path::Path) {
        let log = ErrorLog::new(self.paths.errors_log());
        for (path, e) in datarig_core::results::spill::sweep(state).failed {
            log.record("results.spill_sweep", &Fault::io_at(&e, &path));
        }
    }

    /// Write the workspace state soon (after an edit).
    pub(super) fn mark_workspace(&mut self) {
        if self.state_dir().is_some() && self.workspace_due.is_none() {
            self.workspace_due = Some(self.now() + AUTOSAVE);
        }
    }

    /// The active tab's text changed: save it within [`AUTOSAVE`]. A text larger than
    /// [`LARGE_TEXT`] is saved once typing pauses for [`AUTOSAVE`], and at most
    /// [`AUTOSAVE_LARGE`] after its oldest unsaved edit, so typing in it does not write (and
    /// flush to disk) the whole file every second.
    pub(super) fn edited(&mut self) {
        let now = self.now();
        let persist = self.state_dir().is_some();
        let t = self.tab_mut();
        if t.doc.script.is_some() || persist {
            let first = *t.doc.unsaved_since.get_or_insert(now);
            t.doc.save_due = Some(match t.doc.save_due {
                Some(_) if t.editor.len_bytes() > LARGE_TEXT => (now + AUTOSAVE).min(first + AUTOSAVE_LARGE),
                Some(due) => due,
                None => now + AUTOSAVE,
            });
        }
        self.mark_workspace();
    }

    /// Save tab `id`'s text: a console to the state directory, a saved query to its file (never
    /// over a file that changed on disk since it was read).
    pub(super) fn save_tab(&mut self, id: TabId) -> Saved {
        let state = self.state_dir();
        let retry = self.now() + AUTOSAVE;
        let Some(t) = self.tabs.get_mut(id) else { return Saved::Ok };
        t.doc.save_due = None;
        t.doc.unsaved_since = None;
        // A table tab's query and a DDL tab's text are not texts anybody wrote: no file.
        if !t.is_query() {
            return Saved::Ok;
        }
        let version = t.editor.version();
        let text = t.editor.text();
        let result = match (&t.doc.script, &self.scripts) {
            (Some(path), Some(store)) => {
                if t.doc.conflict {
                    return Saved::Conflict;
                }
                store.save(path, &text, t.doc.stamp).map(Some)
            }
            (Some(_), None) => return Saved::Ok,
            (None, _) => match state {
                Some(dir) => workspace::write_console(&dir, &t.doc.console_id, &text)
                    .map(|()| None)
                    .map_err(|e| SaveError::Io(e.kind(), e.to_string())),
                None => return Saved::Ok,
            },
        };
        match result {
            Ok(stamp) => {
                t.doc.saved = text;
                t.doc.saved_version = Some(version);
                t.doc.written = true;
                if stamp.is_some() {
                    t.doc.stamp = stamp;
                }
                t.doc.save_error = None;
                Saved::Ok
            }
            Err(SaveError::Conflict | SaveError::Missing) => {
                let missing = self
                    .tabs
                    .get(id)
                    .and_then(|t| t.script())
                    .is_some_and(|p| self.scripts.as_ref().is_some_and(|s| s.stamp(p).is_none()));
                self.script_conflict(id, missing);
                Saved::Conflict
            }
            Err(e) => {
                let reason = self.save_reason(&e);
                if let SaveError::Io(kind, detail) = &e {
                    ErrorLog::new(self.paths.errors_log())
                        .record("scripts.save_failed", &Fault::new(FaultKind::Io(*kind), detail.clone()));
                }
                let Some(t) = self.tabs.get_mut(id) else { return Saved::Ok };
                let first = t.doc.save_error.is_none();
                t.doc.save_error = Some(reason.clone());
                // Tried again a moment later.
                t.doc.save_due = Some(retry);
                let error = self.reason_text(&reason);
                if first {
                    self.flash(Notice::new(Msg::ScriptsSaveFailed { error: error.clone() }, Level::Error));
                }
                Saved::Failed(error)
            }
        }
    }

    /// Tabs whose text should be on disk and is not: the last save failed or one is still due
    /// (conflicts are [`App::conflicted`]'s). A console counts only where consoles are kept
    /// (not in a second instance, not without a state directory).
    pub(super) fn unsaved(&self, only: Option<TabId>) -> usize {
        let kept = |t: &Tab| if t.doc.script.is_some() { self.scripts.is_some() } else { self.state_dir().is_some() };
        self.tabs.iter().filter(|t| only.is_none_or(|o| o == t.id) && !t.doc.conflict && t.dirty() && kept(t)).count()
    }

    /// Why the first of the [`App::unsaved`] tabs was not saved, if a save failed.
    pub(super) fn unsaved_error(&self, only: Option<TabId>) -> Option<Msg> {
        self.tabs
            .iter()
            .filter(|t| only.is_none_or(|o| o == t.id) && !t.doc.conflict)
            .find_map(|t| t.doc.save_error.as_ref())
            .map(|reason| Msg::ScriptsSaveFailed { error: self.reason_text(reason) })
    }

    /// The index binds saved query tab `id` to its profile, or keeps the binding to a
    /// profile that is unknown right now.
    pub(super) fn sync_binding(&mut self, id: TabId) {
        let Some((Some(path), profile)) = self.tabs.get(id).map(|t| (t.doc.script.clone(), t.saved_profile())) else {
            return;
        };
        if let Some(store) = self.scripts.as_mut()
            && store.binding(&path) != profile
            && store.stamp(&path).is_some()
            && let Err(e) = store.bind(&path, profile)
        {
            let error = self.io_text(&e);
            self.flash(Notice::new(Msg::ScriptsIndexSaveFailed { error }, Level::Error));
        }
    }

    /// Timers of autosave: tabs whose save is due, then the workspace state.
    pub(super) fn autosave_tick(&mut self, now: Instant) {
        let due: Vec<TabId> =
            self.tabs.iter().filter(|t| t.doc.save_due.is_some_and(|d| now >= d)).map(|t| t.id).collect();
        for id in due {
            self.save_tab(id);
        }
        if self.workspace_due.is_some_and(|d| now >= d) {
            self.save_workspace_with(false);
        }
    }

    /// An autosave is waiting for its time.
    pub(super) fn save_pending(&self) -> bool {
        self.workspace_due.is_some() || self.tabs.iter().any(|t| t.doc.save_due.is_some())
    }

    /// After every input: the focus stays on a drawn pane (a move of it ends a zoom or shows the
    /// hidden explorer), a change of focus into the explorer rescans the saved queries, and a
    /// restored (or opened) tab whose profile is not connected connects once its editor or
    /// results have the focus; a tab only made active while the explorer keeps the
    /// focus connects nothing.
    pub(super) fn after_input(&mut self) {
        // The input moved the focus itself (not one of the corrections below).
        let moved = self.focus != self.last_focus;
        // Without a tab only the explorer can have the focus (the welcome panel stands in for
        // the tabs while there is no profile).
        if self.tabs.is_empty() && !self.profiles.is_empty() {
            self.focus = Focus::Tree;
        }
        // The inspector went away (hidden, a result without rows): its grid keeps the focus.
        if self.focus == Focus::Inspector && !self.inspector_shown() {
            self.focus = Focus::Results;
        }
        // A pane that is not drawn (a table tab's editor, a hidden results pane) never keeps
        // the focus.
        self.settle_panes(moved);
        if self.focus != self.last_focus {
            self.last_focus = self.focus;
            if self.focus == Focus::Tree {
                self.refresh_scripts();
            }
        }
        if self.focus == Focus::Tree || self.overlays.modal() || self.profiles.is_empty() {
            return;
        }
        let t = self.tab_mut();
        if !std::mem::take(&mut t.doc.lazy_connect) {
            return;
        }
        if let Some(p) = t.profile
            && self.conns.state(p) == NodeState::Disconnected
        {
            self.connect(p);
        }
    }
}

/// A fault as the UI says it: a short reason for a message's `{error}` (never the raw text).
pub fn fault_reason(f: &Fault) -> Msg {
    match &f.kind {
        FaultKind::Io(kind) => io_reason(*kind),
        FaultKind::HostNotFound => Label::DbHostNotFound.into(),
        FaultKind::Toml { line: Some(line) } => Msg::FaultTomlLine { line: line.to_string() },
        FaultKind::Toml { line: None } => Label::FaultToml.into(),
        FaultKind::Shape { key } => Msg::FaultShape { key: key.clone() },
        FaultKind::Keychain(k) => match k {
            KeychainFault::Locked => Label::FaultKeychainLocked,
            KeychainFault::NoStore => Label::FaultKeychainNoStore,
            KeychainFault::Damaged => Label::FaultKeychainDamaged,
            KeychainFault::Refused => Label::FaultKeychainRefused,
            KeychainFault::Ambiguous => Label::FaultKeychainAmbiguous,
            KeychainFault::Mismatch => Label::FaultKeychainMismatch,
            KeychainFault::Failure => Label::FaultKeychainFailure,
            KeychainFault::NoAnswer => Label::FaultKeychainNoAnswer,
        }
        .into(),
        FaultKind::NotPrivate => Label::FaultNotPrivate.into(),
        FaultKind::Other => Label::FaultOther.into(),
    }
}
