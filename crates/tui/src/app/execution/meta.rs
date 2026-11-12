//! The auxiliary and meta sessions: opening them and their events (tree, completion, checks).

use crate::app::*;
use datarig_core::driver::DbError;
use datarig_core::driver::structure::TableStructure;
use datarig_core::fault::ErrorLog;
use datarig_core::sql::values::check_values;

impl App {
    /// Open profile `id`'s metadata session in database `database` unless it has one (step
    /// 2.7.1): once the profile is connected (it needs the password the profile connected
    /// with). The profile's own database has its main metadata session.
    ///
    /// Each call is a use (the session stays while used); one closed for being
    /// idle, failed or lost opens again (what it read stays until the new one reads it again).
    pub(in crate::app) fn ensure_aux(&mut self, id: ProfileId, database: &str) {
        let now = self.now();
        let known = self.conns.aux(id, database).map(|a| (a.id, a.session.is_some()));
        if let Some((aux, _)) = known
            && let Some(a) = self.conns.aux_by_id(aux)
        {
            a.used = now;
        }
        if known.is_some_and(|(_, open)| open) {
            return;
        }
        let Some(cfg) = self.conns.get(id).filter(|c| c.connected).and_then(|c| c.resolved.clone()) else { return };
        let aux = match known {
            Some((old, _)) => match self.conns.renew_aux(old) {
                Some(aux) => aux,
                None => return,
            },
            None => self.conns.add_aux(id, database, now),
        };
        let context = datarig_core::driver::SessionContext { database: Some(database.to_string()), schema: None };
        let session = self.open_session(&cfg, SessionRole::Meta, EventTarget::Aux(aux), aux, context);
        if let Some(a) = self.conns.aux_by_id(aux) {
            a.session = session;
        }
    }

    /// An event of a metadata session in another database: its schemas, catalog and keys.
    pub(super) fn aux_event(&mut self, id: u64, ev: DbEvent) {
        let text = self.event_error_text(&ev);
        // The error log gets the underlying error, not the UI's words for it (which may point
        // to the error log itself).
        let raw = match &ev {
            DbEvent::ConnectFailed { error, .. } | DbEvent::Lost { error } => error.raw().into_owned(),
            _ => String::new(),
        };
        let Some(a) = self.conns.aux_by_id(id) else { return };
        let (profile, database) = (a.profile, a.database.clone());
        if let DbEvent::Ddl { id: request, result } = ev {
            return self.ddl_answered(profile, Some(&database), request, result);
        }
        if let DbEvent::TableKeys { id: request, result } = ev {
            return self.order_keys_answered(profile, Some(&database), request, result);
        }
        if matches!(ev, DbEvent::ConnectFailed { .. } | DbEvent::Lost { .. }) {
            self.ddl_stop(profile, Some(Some(&database)), crate::app::tabs::DdlState::Failed(text.clone()), true);
            self.table_keys_gone(profile, Some(&database), true);
        }
        let Some(a) = self.conns.aux_by_id(id) else { return };
        match ev {
            // It opens again on the next use; why it failed is said, and where it
            // was not read yet, what the picker and the explorer show.
            DbEvent::ConnectFailed { .. } | DbEvent::Lost { .. } => {
                if let Some(s) = a.session.take() {
                    s.close();
                }
                // What it was asked for never comes: asked again on the next use.
                a.checks.retain(|_, c| *c != TableChecks::Asked);
                if !matches!(a.schemas, Some(Ok(_))) {
                    a.schemas = Some(Err(text.clone()));
                }
                a.tree.schemas_loading = false;
                let (database, profile) = (a.database.clone(), a.profile);
                // The whole reason (the explorer's note and the status bar cut it):
                // the error log, and the app's Messages, one entry per database.
                let why = if raw.is_empty() { &text } else { &raw };
                let detail = datarig_core::fault::Fault::other(format!("{database}: {why}"));
                ErrorLog::new(self.paths.errors_log()).record("aux.failed", &detail);
                let m = Notice::new(Msg::AuxFailed { database: database.clone(), error: text }, Level::Error);
                self.forget_aux_failure(&database);
                self.notices.push(m.clone());
                self.status = Some(m);
                self.use_answered(profile, crate::app::cmdline::UseAsk::Schemas(database), false);
            }
            DbEvent::Schemas(Ok(names)) => {
                a.tree.set_schemas(names.clone());
                a.schemas = Some(Ok(names));
                // It opened: what was said about its last failure goes.
                let (database, profile) = (a.database.clone(), a.profile);
                self.forget_aux_failure(&database);
                self.use_answered(profile, crate::app::cmdline::UseAsk::Schemas(database), true);
            }
            DbEvent::Schemas(Err(_)) => {
                a.tree.schemas_loading = false;
                a.schemas = Some(Err(text));
                let (database, profile) = (a.database.clone(), a.profile);
                self.use_answered(profile, crate::app::cmdline::UseAsk::Schemas(database), false);
            }
            DbEvent::Objects { schema, result } => {
                a.tree.set_objects(&schema, result.map_err(|_| text));
                let (profile, database) = (a.profile, a.database.clone());
                self.revealed(profile, Some(&database), &schema);
            }
            DbEvent::Structure { schema, table, result } => {
                let (profile, database) = (a.profile, a.database.clone());
                self.edit_structure_came(profile, Some(&database), &schema, &table, result.as_deref(), &text);
                let came = self.checks_came(profile, Some(&database), &schema, &table, result.as_deref());
                // A failure the explorer does not wait for (completion's) is not its.
                if let Some(a) = self.conns.aux_by_id(id)
                    && (result.is_ok() || a.tree.structure_awaited(&schema, &table))
                {
                    a.tree.set_structure(&schema, &table, result.map_err(|_| text));
                }
                if came {
                    self.values_came(profile, Some(&database), &schema, &table);
                }
            }
            DbEvent::Catalog(Ok(cat)) => {
                a.catalog = cat;
                a.checks.clear();
            }
            DbEvent::Keys(Ok(keys)) => a.keys = Keys::Loaded(keys),
            DbEvent::Keys(Err(_)) => a.keys = Keys::Failed(text),
            _ => {}
        }
    }

    /// Database `database`'s metadata session opened (or failed again): the entry of its last
    /// failure leaves the Messages, and the status bar when it says that failure.
    fn forget_aux_failure(&mut self, database: &str) {
        let of = |n: &Notice| matches!(&n.msg, Msg::AuxFailed { database: d, .. } if d == database);
        self.notices.retain(|n| !of(n));
        if self.status.as_ref().is_some_and(of) {
            self.status = None;
        }
    }

    pub(super) fn meta_event(&mut self, id: ProfileId, ev: DbEvent) {
        let Some(name) = self.profile(id).map(|p| p.name.clone()) else { return };
        let text = self.event_error_text(&ev);
        match ev {
            DbEvent::Encryption(e) => self.conns.entry(id).encryption = Some(e),
            DbEvent::Connected => self.on_connected(id),
            // The profile's sessions are read-only per transaction only: said once it connected.
            DbEvent::ReadOnlyPerTransaction => {
                self.status = Some(Notice::new(Msg::DbReadOnlyPerTransaction { name }, Level::Info));
            }
            // Only a query session has start-up statements.
            DbEvent::SessionPerTransaction { .. } => {}
            DbEvent::ConnectFailed { error, auth } => {
                let had_password =
                    self.conns.get(id).and_then(|c| c.connecting.as_ref()).is_none_or(|a| a.had_password);
                self.save_on_connect = None;
                // A password typed in the prompt that MySQL refused is not asked for again: the
                // account may require TLS, which no password fixes.
                let typed_refused =
                    matches!(error, DbError::AccessDenied(_)) && self.conns.get(id).is_some_and(|c| c.prompted);
                // The server asked for a password and none was sent: say so plainly.
                let missing = auth && !typed_refused && !had_password && error.raw().contains("password");
                let failed = Notice::new(Msg::ConnFailed { error: text }, Level::Error);
                let reason = if !missing {
                    failed.clone()
                } else if self.secrets.unavailable.is_some() {
                    Notice::new(Label::PromptPasswordNoKeychain, Level::Warning)
                } else {
                    Notice::new(Label::PromptPasswordMissing, Level::Warning)
                };
                // Ask again, except for a command or an environment variable (those are fixed
                // where they come from) and a refused typed password.
                let ask = self.profile(id).cloned().filter(|c| {
                    auth && !typed_refused && !matches!(c.source().kind(), SourceKind::Command | SourceKind::Env)
                });
                // What waited for this attempt waits for the next one when the prompt asks;
                // otherwise it is dropped and its tabs say so.
                let (console, expand) = self.conns.get(id).map_or((None, false), |c| (c.console, c.expand_on_connect));
                let pending = match ask {
                    Some(_) => self.conns.get_mut(id).map(|c| std::mem::take(&mut c.pending)).unwrap_or_default(),
                    None => Vec::new(),
                };
                let ddl = if ask.is_some() { self.ddl_waiting(id) } else { Vec::new() };
                let keys = if ask.is_some() { self.table_keys_waits(id) } else { Vec::new() };
                self.attempt_failed(id, reason.clone());
                self.ddl_wait_again(&ddl);
                self.table_keys_wait_again(keys);
                // A tunnel stays open only for the password asked for now.
                if ask.is_none() {
                    self.close_tunnel(id);
                }
                if let Some(c) = ask {
                    let pc = self.conns.entry(id);
                    pc.console = console;
                    pc.expand_on_connect = expand;
                    pc.pending = pending;
                    self.open_prompt(&c, reason.clone(), PromptPurpose::Connect);
                }
                self.status = Some(failed);
            }
            DbEvent::Schemas(Ok(names)) => {
                self.conns.entry(id).tree.set_schemas(names);
                let own = self.own_database(id);
                self.use_answered(id, crate::app::cmdline::UseAsk::Schemas(own), true);
            }
            DbEvent::Schemas(Err(_)) => {
                self.conns.entry(id).tree.schemas_loading = false;
                self.status = Some(Notice::new(Msg::QueryError { error: text }, Level::Error));
                let own = self.own_database(id);
                self.use_answered(id, crate::app::cmdline::UseAsk::Schemas(own), false);
            }
            DbEvent::Objects { schema, result } => {
                self.conns.entry(id).tree.set_objects(&schema, result.map_err(|_| text));
                self.revealed(id, None, &schema);
            }
            DbEvent::Structure { schema, table, result } => {
                self.export_structure_came(id, &schema, &table, result.as_deref());
                self.edit_structure_came(id, None, &schema, &table, result.as_deref(), &text);
                let came = self.checks_came(id, None, &schema, &table, result.as_deref());
                // A failure the explorer does not wait for (completion's) is not its.
                let tree = &mut self.conns.entry(id).tree;
                if result.is_ok() || tree.structure_awaited(&schema, &table) {
                    tree.set_structure(&schema, &table, result.map_err(|_| text));
                }
                if came {
                    self.values_came(id, None, &schema, &table);
                }
            }
            DbEvent::Catalog(Ok(cat)) => {
                let c = self.conns.entry(id);
                c.catalog = cat;
                c.catalog_error = None;
                c.checks.clear();
            }
            DbEvent::Catalog(Err(_)) => {
                // Completion and the explorer's columns keep what was read before, if anything;
                // the columns say it could not be read.
                self.conns.entry(id).catalog_error = Some(text.clone());
                self.status = Some(Notice::new(Msg::CatalogFailed { error: text }, Level::Error));
            }
            DbEvent::Keys(Ok(keys)) => self.conns.entry(id).keys = Keys::Loaded(keys),
            // Only a query session sets its path per transaction.
            // Only query sessions keep prepared statements.
            DbEvent::ContextPerTransaction | DbEvent::StatementCacheOff => {}
            DbEvent::Databases(result) => {
                let c = self.conns.entry(id);
                let ok = result.is_ok();
                c.databases = Some(result.map_err(|_| text));
                c.databases_asked = false;
                self.use_answered(id, crate::app::cmdline::UseAsk::Databases, ok);
            }
            DbEvent::Context { database, .. } => self.conns.entry(id).database = Some(database),
            DbEvent::Keys(Err(_)) => {
                self.conns.entry(id).keys = Keys::Failed(text.clone());
                self.status = Some(Notice::new(Msg::KeysFailed { error: text }, Level::Warning));
            }
            DbEvent::Lost { error } => {
                // Lost with its tunnel: the tunnel's words; closed by the server for being idle,
                // that.
                let lost = self.conns.get(id).and_then(|c| c.tunnel_lost.clone());
                let m = lost.unwrap_or_else(|| match error {
                    DbError::IdleSessionTimeout(_) => {
                        Notice::new(Msg::ConnIdleClosedByServer { name, error: text }, Level::Info)
                    }
                    _ => Notice::new(Msg::ConnLost { name, error: text }, Level::Error),
                });
                let why = self.i18n.msg(&m.msg).to_string();
                self.ddl_stop(id, Some(None), crate::app::tabs::DdlState::Failed(why), true);
                self.table_keys_gone(id, None, true);
                let c = self.conns.entry(id);
                c.meta = None;
                c.connected = false;
                c.error = Some(m.clone());
                // What it was asked for never comes: asked again on the next use.
                c.checks.retain(|_, c| *c != TableChecks::Asked);
                self.status = Some(m);
            }
            // The mode a new session of the profile starts in (MySQL): a tab's first run is
            // checked in it before the tab's own session says.
            DbEvent::Language(lang) => self.set_profile_language(id, lang),
            // The plan format its server gives (the plan actions follow it).
            DbEvent::Explain(format) => self.conns.entry(id).explain = Some(format),
            // The edits its server takes.
            DbEvent::Edit(format) => self.conns.entry(id).edit = Some(format),
            // Whether its server reads a time zone offset in a literal.
            DbEvent::ZoneOffsets(on) => self.conns.entry(id).zone_offsets = Some(on),
            // A query session's own (copying its values as SQL needs it).
            DbEvent::SessionValues(_) | DbEvent::ValuesNotKnown { .. } => {}
            // A monitor session's own.
            DbEvent::Monitor(_)
            | DbEvent::MonitorSnapshot { .. }
            | DbEvent::MonitorQueryText { .. }
            | DbEvent::MonitorReplication { .. }
            | DbEvent::MonitorActionDone { .. }
            | DbEvent::MonitorFollowUp { .. }
            | DbEvent::ServerSession { .. } => {}
            // Statement events never come from the metadata session.
            DbEvent::Info { .. }
            | DbEvent::Released { .. }
            | DbEvent::Page { .. }
            | DbEvent::Done { .. }
            | DbEvent::Failed { .. }
            | DbEvent::TxOpen(_)
            | DbEvent::TxAborted(_)
            | DbEvent::Block(_)
            | DbEvent::Counted { .. }
            | DbEvent::RepeatChecked { .. }
            | DbEvent::Started { .. }
            | DbEvent::StepRows { .. }
            | DbEvent::Finished { .. }
            | DbEvent::Stopping { .. }
            | DbEvent::CancelRefused { .. }
            | DbEvent::Exported { .. }
            | DbEvent::ManualBegun { .. }
            | DbEvent::ManualRolledBack { .. }
            | DbEvent::RunRolledBack { .. }
            | DbEvent::TxEnded { .. }
            | DbEvent::TxClosedBy { .. }
            | DbEvent::Autocommit(_)
            | DbEvent::Isolation(_)
            | DbEvent::Applied { .. } => {}
            DbEvent::Ddl { id: request, result } => self.ddl_answered(id, None, request, result),
            DbEvent::TableKeys { id: request, result } => self.order_keys_answered(id, None, request, result),
        }
    }

    /// The language a new session of profile `p` starts in: its tabs without a session's word
    /// of their own read their text in it.
    pub(super) fn set_profile_language(&mut self, p: ProfileId, lang: Language) {
        self.conns.entry(p).language = Some(lang);
        let tabs: Vec<TabId> = self.tabs.iter().filter(|t| t.profile == Some(p)).map(|t| t.id).collect();
        for t in tabs {
            self.sync_tab_language(t);
        }
    }

    /// The structure of `schema.table` in `profile`'s `database` (`None`: its own) came, or why
    /// it could not be read: what its checks allow goes to completion's cache. A server that
    /// reads no structure (too old) has no values to give; another failure leaves the table
    /// not known, asked again after [`CHECKS_RETRY`]. Whether values came.
    fn checks_came(
        &mut self,
        profile: ProfileId,
        database: Option<&str>,
        schema: &str,
        table: &str,
        result: Result<&TableStructure, &DbError>,
    ) -> bool {
        let (dialect, now) = (self.profile_language(Some(profile)).dialect(), self.now());
        let Some(cache) = self.conns.checks_in_mut(profile, database) else { return false };
        let key = (schema.to_string(), table.to_string());
        match result {
            Ok(st) => {
                cache.insert(key, TableChecks::Known(check_values(dialect, &st.checks)));
                true
            }
            Err(DbError::ServerTooOld | DbError::NotSupported) => {
                cache.insert(key, TableChecks::Known(Vec::new()));
                false
            }
            Err(_) => {
                cache.insert(key, TableChecks::Failed(now));
                false
            }
        }
    }

    /// The checks' values of `schema.table` in `profile`'s `database` came: completion in the
    /// active tab that waits for them is made again, if the tab is still bound as it was, its
    /// editor in Insert mode and its cursor in the same literal. Whatever else waited for them
    /// is dropped.
    fn values_came(&mut self, profile: ProfileId, database: Option<&str>, schema: &str, table: &str) {
        let generation = self.conns.get(profile).map(|c| c.generation);
        let other = self.other_database(self.tab()).map(str::to_string);
        let t = self.tab();
        let Some(w) = t.values_wait.clone() else { return };
        // The text it waits in still has the keys.
        let editing = !self.modal_open()
            && match w.owner {
                PopupOwner::Editor => self.focus == Focus::Editor && self.tab().editor.mode == Mode::Insert,
                PopupOwner::Filter(f) => self.filter_line_focused() && self.filter_line_field() == Some(f),
            };
        if w.profile != profile || w.database.as_deref() != database || w.schema != schema || w.table != table {
            return;
        }
        let current = t.binding == w.binding
            && t.profile == Some(profile)
            && generation == Some(w.generation)
            && other == w.database;
        self.tab_mut().values_wait = None;
        if current && editing {
            match w.owner {
                PopupOwner::Editor => self.complete_here(false, Some(w.literal)),
                PopupOwner::Filter(f) => {
                    self.filter_complete(false, Some(w.literal));
                    if let Some(p) = self.tab_mut().popup.as_mut().filter(|p| p.owner == PopupOwner::Filter(f)) {
                        p.touched |= w.asked;
                    }
                }
            }
        }
    }
}
