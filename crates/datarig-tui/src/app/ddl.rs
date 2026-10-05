//! Show DDL: an object's `CREATE` statements, read from the catalog by the metadata session of
//! its profile and database (`DbCommand::LoadDdl`, `Capabilities::ddl`) and shown as SQL in a
//! read-only tab of its own ([`TabKind::Ddl`]).
//!
//! A DDL tab is opened from the explorer (`D` on a table, view, materialized view, index or
//! trigger, `F` on a trigger for its function; the node's menu) or with `:ddl [name]`. It
//! reads its object when it opens and again on `r` or the run key; a restored or reopened one
//! waits for that. Each read is a request of its own: the answer of an older request, of
//! another profile or database, or for a tab that went is dropped. A locked object shows only
//! that it is locked, never a part of its DDL. The text can be moved in, selected, searched
//! and yanked; a command that would change it says it is read-only. `o` opens the text in a
//! new console on the same profile and database.

use super::explorer::RowKind;
use super::tabs::{DdlState, DdlTab};
use super::*;
use datarig_core::driver::ddl::{DdlObject, DdlSource};
use datarig_core::driver::structure::StructureGroup;
use datarig_core::driver::{DbError, SessionContext};
use datarig_core::sql::ddl::ddl_text;

impl App {
    /// Profile `id`'s driver reads an object's DDL (`Capabilities::ddl`).
    pub(super) fn ddl_on(&self, id: ProfileId) -> bool {
        self.profile(id).and_then(|p| self.driver(&p.driver)).is_some_and(|d| d.capabilities().ddl)
    }

    /// The object explorer row `row` stands for, and where: its profile and database (`None`: the
    /// profile's own). A table, view or materialized view, or anything under one, is that
    /// relation; an index or a trigger (or a line under it) is that index or trigger, and with
    /// `function` a trigger is the function it calls. `None` for anything else.
    pub(super) fn ddl_object_of_row(
        &self,
        row: &RowKind,
        function: bool,
    ) -> Option<(ProfileId, Option<String>, DdlObject)> {
        let (id, database, node): (ProfileId, Option<String>, _) = match row {
            RowKind::Node(id, n) => (*id, None, *n),
            RowKind::AuxNode(id, db, n) => (*id, Some(db.clone()), *n),
            _ => return None,
        };
        let tree = match database.as_deref() {
            None => &self.conns.get(id)?.tree,
            Some(db) => &self.conns.aux(id, db)?.tree,
        };
        use crate::widgets::tree::Node as N;
        let (i, g, j, item) = match node {
            N::Object(i, g, j) | N::Column(i, g, j, _) | N::NoColumns(i, g, j) | N::StructNote(i, g, j) => {
                (i, g, j, None)
            }
            N::StructGroup(i, g, j, _) => (i, g, j, None),
            N::StructItem(i, g, j, sg, k)
            | N::StructDetail(i, g, j, sg, k, _)
            | N::StructColumns(i, g, j, sg, k)
            | N::StructColumn(i, g, j, sg, k, _) => (i, g, j, Some((sg, k))),
            N::Schema(_) | N::Group(..) | N::Loading(_) | N::Empty(_) | N::Error(_) => return None,
        };
        let (schema, name) = tree.object_name(i, g, j)?;
        let structure = || tree.object_view(i, g, j).and_then(|v| v.loaded());
        let object = match item {
            Some((StructureGroup::Triggers, k)) => {
                let trigger = structure()?.triggers.get(k)?.name.clone();
                match function {
                    true => DdlObject::TriggerFunction { schema, table: name, trigger },
                    false => DdlObject::Trigger { schema, table: name, name: trigger },
                }
            }
            _ if function => return None,
            Some((StructureGroup::Indexes, k)) => {
                DdlObject::Index { schema, name: structure()?.indexes.get(k)?.name.clone() }
            }
            _ => DdlObject::Relation { schema, name },
        };
        Some((id, database, object))
    }

    /// `D` (`F`: the trigger's function) in the explorer: the DDL of the object under the
    /// cursor, in its tab.
    pub(super) fn explorer_show_ddl(&mut self, function: bool) {
        let Some(row) = self.explorer_row() else { return };
        match self.ddl_object_of_row(&row.kind, function) {
            Some((id, database, object)) => self.open_ddl(id, database, object),
            None => {
                let l = if function { Label::DdlNoFunction } else { Label::DdlNoObject };
                self.flash(Notice::new(l, Level::Info));
            }
        }
    }

    /// The DDL of `object` of profile `id` in `database` (`None`: the profile's own) in its own
    /// tab: an open one of the same profile, database and object becomes active (read again
    /// only when it is not read yet), else a new one opens and reads it. The focus stays where
    /// it is, as when a table opens.
    pub(super) fn open_ddl(&mut self, id: ProfileId, database: Option<String>, object: DdlObject) {
        if !self.ddl_on(id) {
            return self.flash(Notice::new(Label::DdlUnsupported, Level::Info));
        }
        let context = self.session_context(id, database, None);
        if let Some(t) = self.tabs.find_ddl(id, context.database.as_deref(), &object) {
            self.switch_tab(|m| m.position(t).is_some_and(|i| m.activate(i)));
            let unread = self
                .tabs
                .get(t)
                .and_then(|tab| tab.doc.ddl.as_ref())
                .is_some_and(|d| !matches!(d.state, DdlState::Loaded | DdlState::Loading | DdlState::Connecting));
            if unread {
                self.load_ddl(t);
            }
            return;
        }
        self.leave_tab();
        let tab = self.tabs.open(TabKind::Ddl, Some(id), Editor::read_only(""));
        if let Some(t) = self.tabs.get_mut(tab) {
            t.doc.ddl = Some(DdlTab::new(object));
            t.doc.written = true;
            t.context = context;
        }
        self.entered_tab();
        self.fix_focus();
        self.load_ddl(tab);
    }

    /// Read DDL tab `tab`'s object (again): on the metadata session of its profile and database,
    /// once the profile is connected (it connects first when it is not).
    pub(super) fn load_ddl(&mut self, tab: TabId) {
        let Some(t) = self.tabs.get(tab) else { return };
        let (Some(id), Some(d)) = (t.profile, t.doc.ddl.as_ref()) else {
            // No connection to read it on: the tab says so.
            return self.flash(Notice::new(Label::DdlNoConnection, Level::Warning));
        };
        let (object, database) = (d.object.clone(), t.context.database.clone());
        // A tab restored on a profile whose driver no longer reads DDL.
        if !self.ddl_on(id) {
            let why = self.i18n.label(Label::DdlUnsupported).to_string();
            return self.set_ddl(tab, DdlState::Failed(why), None);
        }
        if !self.conns.is_connected(id) {
            // Waiting first: an attempt that fails at once (or a password prompt the user
            // cancels) ends the wait.
            self.set_ddl(tab, DdlState::Connecting, None);
            if self.conns.state(id) != NodeState::Connecting {
                self.connect(id);
            }
            return;
        }
        self.ddl_seq += 1;
        let request = self.ddl_seq;
        if let Some(d) = self.tabs.get_mut(tab).and_then(|t| t.doc.ddl.as_mut()) {
            d.state = DdlState::Loading;
            d.request = Some(request);
        }
        let cmd = DbCommand::LoadDdl { id: request, object };
        if let Some(db) = &database {
            self.ensure_aux(id, db);
        }
        let session = match &database {
            None => self.conns.get(id).and_then(|c| c.meta.as_ref()),
            Some(db) => self.conns.aux(id, db).and_then(|a| a.session.as_ref()),
        };
        // Sent, or said: a request that went nowhere never leaves the tab waiting.
        match session {
            Some(s) => s.send(cmd),
            None => {
                let why = self.i18n.label(Label::DdlNoSession).to_string();
                self.set_ddl(tab, DdlState::Failed(why), None);
            }
        }
    }

    /// DDL tab `tab` is in `state` now (no request waits any more), with `text` in its editor
    /// when it was read.
    fn set_ddl(&mut self, tab: TabId, state: DdlState, read: Option<(String, String)>) {
        let Some(t) = self.tabs.get_mut(tab) else { return };
        let Some(d) = t.doc.ddl.as_mut() else { return };
        // Waiting for the connection keeps what was read until the new answer replaces it;
        // nothing of an earlier read stays next to a lock or a failure.
        let clear = !matches!(state, DdlState::Connecting);
        d.request = None;
        d.state = state;
        if let Some((name, text)) = read {
            d.name = Some(name);
            // The cursor stays where it was when the same object is read again.
            let (row, col, top) = (t.editor.row, t.editor.col, t.editor.top);
            t.editor = Editor::read_only(&text);
            t.editor.row = row.min(t.editor.lines.len() - 1);
            t.editor.col = col;
            t.editor.top = top.min(t.editor.row);
            t.doc.saved = text;
            t.doc.written = true;
        } else if clear {
            t.editor = Editor::read_only("");
            t.doc.saved.clear();
        }
    }

    /// The answer to DDL request `request`, from profile `id`'s metadata session in `database`
    /// (`None`: the profile's own): its tab shows it, or why it could not be read. An answer no
    /// tab waits for (an older request, another binding, a tab that went) is dropped.
    pub(super) fn ddl_answered(
        &mut self,
        id: ProfileId,
        database: Option<&str>,
        request: u64,
        result: Result<DdlSource, DbError>,
    ) {
        let waiting = self.tabs.iter().find(|t| {
            t.profile == Some(id)
                && t.context.database.as_deref() == database
                && t.doc.ddl.as_ref().is_some_and(|d| d.request == Some(request))
        });
        let Some(tab) = waiting.map(|t| t.id) else { return };
        match result {
            Ok(source) => {
                let name = source.label();
                self.set_ddl(tab, DdlState::Loaded, Some((name, ddl_text(&source))));
            }
            Err(DbError::Locked) => self.set_ddl(tab, DdlState::Locked, None),
            Err(e) => {
                let text = self.db_error_text(&e);
                self.set_ddl(tab, DdlState::Failed(text), None);
            }
        }
    }

    /// Profile `id`'s metadata session in `database` (every one of the profile with `None`)
    /// ended, failed or was closed: its DDL tabs that wait for an answer (and with `connecting`
    /// those that wait for the connection) stop waiting, in `state` (why, or not read), and
    /// read again on `r`.
    pub(super) fn ddl_stop(
        &mut self,
        id: ProfileId,
        database: Option<Option<&str>>,
        state: DdlState,
        connecting: bool,
    ) {
        let tabs: Vec<TabId> =
            self.tabs
                .iter()
                .filter(|t| t.profile == Some(id) && database.is_none_or(|db| t.context.database.as_deref() == db))
                .filter(|t| {
                    t.doc.ddl.as_ref().is_some_and(|d| {
                        d.state == DdlState::Loading || (connecting && d.state == DdlState::Connecting)
                    })
                })
                .map(|t| t.id)
                .collect();
        for tab in tabs {
            self.set_ddl(tab, state.clone(), None);
        }
    }

    /// The DDL tabs that wait for profile `id` to connect.
    pub(super) fn ddl_waiting(&self, id: ProfileId) -> Vec<TabId> {
        let waits = |t: &&super::tabs::Tab| t.doc.ddl.as_ref().is_some_and(|d| d.state == DdlState::Connecting);
        self.tabs.iter().filter(|t| t.profile == Some(id)).filter(waits).map(|t| t.id).collect()
    }

    /// DDL tabs `tabs` wait for their profile's next attempt (the password it asks for).
    pub(super) fn ddl_wait_again(&mut self, tabs: &[TabId]) {
        for t in tabs {
            self.set_ddl(*t, DdlState::Connecting, None);
        }
    }

    /// Profile `id` connected: its DDL tabs that waited for it read their objects.
    pub(super) fn ddl_connected(&mut self, id: ProfileId) {
        for tab in self.ddl_waiting(id) {
            self.load_ddl(tab);
        }
    }

    /// `o` / "Open in a console" in a DDL tab: its text in a new console on the same profile,
    /// database and schema, with the focus in its editor. Nothing read yet: said.
    pub(super) fn ddl_to_console(&mut self) {
        let t = self.tab();
        let loaded = t.doc.ddl.as_ref().is_some_and(|d| d.state == DdlState::Loaded);
        if !loaded {
            return self.flash(Notice::new(Label::DdlNothingRead, Level::Info));
        }
        let (text, profile, context) = (t.editor.text(), t.profile, t.context.clone());
        self.console_with(&text, profile, context);
    }

    /// A command that would change the DDL tab's text did nothing: said, with the key that
    /// opens the text in a console instead.
    pub(super) fn say_read_only(&mut self) {
        let key = self.key_for(Action::DdlToConsole, Ctx::Ddl);
        self.flash(Notice::new(Msg::DdlReadOnly { key }, Level::Info));
    }

    /// "Copy the DDL": the whole text of the DDL tab to the clipboard.
    pub(super) fn copy_ddl(&mut self) {
        let t = self.tab();
        let loaded = t.doc.ddl.as_ref().is_some_and(|d| d.state == DdlState::Loaded);
        if !loaded {
            return self.flash(Notice::new(Label::DdlNothingRead, Level::Info));
        }
        let (text, count) = (t.editor.text(), t.editor.lines.len() as u64);
        let done = match self.deliver(&text) {
            Ok(m) => Notice::new(Msg::DdlCopied { count, method: self.method_text(m) }, Level::Success),
            Err(msg) => Notice::new(msg, Level::Error),
        };
        self.show_status(done.clone());
        self.flash(done);
    }

    /// `:ddl [name]`: with a name, what it names on the active tab's profile and database,
    /// looked up in the tab's schema; without one, the explorer's object when the explorer
    /// has the focus, else the active table tab's table.
    pub(super) fn ddl_command(&mut self, name: &str) -> Result<(), Notice> {
        if name.is_empty() {
            if self.focus == Focus::Tree
                && let Some(row) = self.explorer_row()
            {
                let (id, database, object) =
                    self.ddl_object_of_row(&row.kind, false).ok_or(Notice::new(Label::DdlNoObject, Level::Error))?;
                self.open_ddl(id, database, object);
                return Ok(());
            }
            let t = self.tab();
            let (Some(table), Some(id), false) = (&t.doc.table, t.profile, self.tabs.is_empty()) else {
                return Err(Notice::new(Label::DdlCommandUsage, Level::Error));
            };
            let object = DdlObject::Relation { schema: table.schema.clone(), name: table.name.clone() };
            let database = t.context.database.clone();
            self.open_ddl(id, database, object);
            return Ok(());
        }
        let t = self.tab();
        let Some(id) = t.profile.filter(|_| !self.tabs.is_empty()) else {
            return Err(Notice::new(Label::DdlNoConnection, Level::Error));
        };
        let SessionContext { database, schema } = t.context.clone();
        let object = DdlObject::Named { name: name.to_string(), schema };
        self.open_ddl(id, database, object);
        Ok(())
    }
}
