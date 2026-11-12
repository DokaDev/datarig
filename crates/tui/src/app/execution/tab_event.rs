//! A tab's driver events turned into results, transaction state and notices.

use crate::app::persist;
use crate::app::runlog::StatementOutcome;
use crate::app::tabs::TxEnd;
use crate::app::*;
use datarig_core::driver::{DbError, ReadIn, ServerEnd, TxOutcome};
use datarig_core::fault::ErrorLog;
use datarig_core::policy::format_timeout;

impl App {
    /// Tab `id`'s query session turned its statement cache off: said in the
    /// run's Messages, and in the status bar once per connection of the profile.
    fn statement_cache_off(&mut self, id: TabId, profile: Option<ProfileId>) {
        let name = profile.and_then(|p| self.profile(p)).map(|p| p.name.clone()).unwrap_or_default();
        let m = Notice::new(Msg::DbStatementCacheOff { name }, Level::Warning);
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.run.notes.push(m.clone());
        }
        if let Some(p) = profile
            && !std::mem::replace(&mut self.conns.entry(p).cache_off_said, true)
        {
            self.status = Some(m);
        }
    }

    /// An event of tab `id`'s query session.
    pub(super) fn tab_event(&mut self, id: TabId, ev: DbEvent) {
        let profile = self.tabs.get(id).and_then(|t| t.profile);
        // Which server session this one is: kept only as the answer to the ask in flight.
        if let DbEvent::ServerSession { request, own } = ev {
            let now = self.now();
            if let Some(t) = self.tabs.get_mut(id)
                && t.exec.own == crate::app::tabs::OwnAsk::Asked(request)
            {
                t.exec.own = crate::app::tabs::OwnAsk::Known(own, now);
            }
            return;
        }
        // A session that failed or was lost is no server session of the tab's any more.
        if matches!(ev, DbEvent::Lost { .. } | DbEvent::ConnectFailed { .. })
            && let Some(t) = self.tabs.get_mut(id)
        {
            t.exec.own = crate::app::tabs::OwnAsk::NotAsked;
        }
        // A run whose cancel the server refused, that ended by itself after all: said in its
        // Messages (it ran to its end, whatever the user asked).
        if let Some(t) = self.tabs.get_mut(id)
            && let Some(r) = t.exec.running
            && r.refused
            && matches!(&ev, DbEvent::Done { id: i, .. } | DbEvent::Page { id: i, columns: Some(_), .. }
                | DbEvent::Failed { id: i, cancelled: false, .. } if *i == r.id)
        {
            t.exec.run.notes.push(Notice::new(Label::QueryRanAfterRefusedCancel, Level::Warning));
        }
        // A cancelled run whose session speaks is not stuck: the last-resort bound starts over.
        let now = self.now();
        if let Some(r) = self.tabs.get_mut(id).and_then(|t| t.exec.running.as_mut())
            && r.cancelling.is_some()
        {
            r.cancelling = Some(now);
            match ev {
                DbEvent::Stopping { id } if id == r.id => r.stopping = true,
                DbEvent::CancelRefused { id } if id == r.id => r.refused = true,
                _ => {}
            }
        }
        if matches!(ev, DbEvent::Stopping { .. } | DbEvent::CancelRefused { .. }) {
            return;
        }
        if matches!(ev, DbEvent::StatementCacheOff) {
            return self.statement_cache_off(id, profile);
        }
        // The plan format of the profile's server: a query session's word is the profile's.
        if let DbEvent::Explain(format) = ev {
            if let Some(p) = profile {
                self.conns.entry(p).explain = Some(format);
            }
            return;
        }
        // Likewise the edits its server takes.
        if let DbEvent::Edit(format) = ev {
            if let Some(p) = profile {
                self.conns.entry(p).edit = Some(format);
            }
            return;
        }
        // The editor and the risk classifier read the text as the session does from now on.
        if let DbEvent::Language(lang) = ev {
            let first = self.tabs.get_mut(id).is_some_and(|t| t.exec.language.replace(lang).is_none());
            // The session's first word is the mode a new session starts in: the profile's other
            // tabs read theirs in it until their own sessions say. A later change (its `SET
            // sql_mode`) is that session's own.
            if let Some(p) = profile.filter(|_| first) {
                self.set_profile_language(p, lang);
            }
            return self.sync_tab_language(id);
        }
        // What the session's values are read in, from now on.
        if let DbEvent::SessionValues(v) = ev {
            if let Some(t) = self.tabs.get_mut(id) {
                // The first values a session says come as it connects, before any statement of
                // it runs (a run that opened it waits for them): they change none. A result read
                // before the session said any is read in values not known all the same.
                let changed = t.exec.values.replace(v.clone()).is_some_and(|old| old != v);
                // While a statement runs, or its result is still read (held): its result is not
                // read in one set of values.
                if changed && (t.exec.running.is_some() || matches!(t.exec.paging, Paging::Open { .. })) {
                    let query = t.exec.query_id;
                    if t.exec.reading.query != query {
                        t.exec.reading = crate::app::tabs::Reading { query, ..Default::default() };
                    }
                    t.exec.reading.moved = true;
                    let (step, answer) = (t.exec.reading.step, t.exec.reading.answer);
                    let read = match step {
                        Some(i) => t.exec.steps.get_mut(&i).map(|p| &mut p.rs),
                        None if answer => t.answer_rows_mut().map(|(rs, _)| rs),
                        None => None,
                    };
                    if let Some(rs) = read {
                        rs.read_in = ReadIn::Changed;
                    }
                }
            }
            return;
        }
        // Server code runs in the statement now running: its next result is read in values not
        // known.
        if let DbEvent::ValuesNotKnown { id: q } = ev {
            if let Some(t) = self.tabs.get_mut(id).filter(|t| t.exec.query_id == q) {
                if t.exec.reading.query != q {
                    t.exec.reading = crate::app::tabs::Reading { query: q, ..Default::default() };
                }
                t.exec.reading.not_known = true;
            }
            return;
        }
        // The mode the lost session read its text in goes with it: the next run, in a new
        // session, is checked in the mode that one starts in.
        if matches!(ev, DbEvent::Lost { .. }) {
            if let Some(t) = self.tabs.get_mut(id) {
                t.exec.language = None;
                t.exec.values = None;
            }
            self.sync_tab_language(id);
        }
        // The run ends (its result, its outcome or its failure), or a transaction ends.
        let current = self.tabs.get(id).map_or(0, |t| t.exec.query_id);
        // The statement of a table tab's query failed (not a page fetched after its first): the
        // line marks the query failed, and where the server says the error is in its filter.
        if let DbEvent::Failed { id: q, error, cancelled: false, .. } = &ev
            && *q == current
        {
            let d = self.tab_dialect(id);
            if let Some(t) = self.tabs.get_mut(id)
                && !t.exec.running.is_some_and(|r| r.fetch)
                && let Some(tq) = t.doc.table_query.as_mut()
            {
                let e = match error {
                    DbError::Server(m) => crate::app::table_query::ServerError::Message(m),
                    _ => crate::app::table_query::ServerError::Other,
                };
                tq.run_failed(*q, &e, d);
            }
        }
        let run_ended = matches!(&ev, DbEvent::Page { id: q, columns: Some(_), .. } | DbEvent::Done { id: q, .. }
            | DbEvent::Failed { id: q, .. } if *q == current);
        let tx_ended = matches!(ev, DbEvent::TxOpen(false));
        // Rows that land for a result of this run: the answer's (`None`) or a statement's
        // before it. Read inside the user's transaction, the result says so.
        let rows_for: Option<Option<usize>> = match &ev {
            DbEvent::Page { id: q, .. } if *q == current => Some(None),
            DbEvent::StepRows { id: q, index, .. } if *q == current => Some(Some(*index)),
            _ => None,
        };
        let connecting = profile.is_some_and(|p| self.conns.state(p) == NodeState::Connecting);
        let null = self.i18n.label(Label::ResultsNull);
        // What a result read now is bound to: its edits are taken only while that holds.
        let bound = self.tabs.get(id).and_then(|t| self.edit_bound(t));
        let now = self.now();
        let text = match &ev {
            // How long it waited, when the session was opened with a limit the app knows.
            DbEvent::Failed { error: DbError::LockTimeout(s), .. } => {
                match self.tabs.get(id).and_then(|t| t.exec.lock_wait) {
                    Some(w) => {
                        let wait = crate::app::sessions::wait_text(w);
                        self.i18n.msg(&Msg::DbLockTimeout { wait, error: s.clone() }).to_string()
                    }
                    None => self.event_error_text(&ev),
                }
            }
            // A start-up statement failed: which one, of what.
            DbEvent::ConnectFailed { error: DbError::SessionSql { index, statement, error }, .. } => {
                self.session_failed_text(profile, *index, statement, &self.db_error_text(error))
            }
            _ => self.event_error_text(&ev),
        };
        // Behind a pooler the start-up statements go in each transaction: said once, with those
        // that were not sent.
        let per_transaction = match &ev {
            DbEvent::SessionPerTransaction { refused } => Some(self.session_per_transaction_notice(profile, refused)),
            _ => None,
        };
        // A statement run again for the user that the allowlist refused (the server's side of
        // it, asked right before): said as a refusal, with the reason.
        let released = self.tabs.get(id).is_some_and(|t| t.exec.paging == Paging::Released);
        let refused = match &ev {
            DbEvent::Failed { error: DbError::NotRepeatable(r), .. } => {
                let why = self.i18n.msg(&crate::app::pages::why(r)).to_string();
                let m = if released { Msg::ResultsPageRefusedNoHold { why } } else { Msg::ResultsPageRefused { why } };
                Some(Notice::new(m, Level::Warning))
            }
            _ => None,
        };
        // A procedure that commits by itself failed as the first statement of the transaction
        // opened for it (manual commit): once that is rolled back, the user is asked whether to
        // run it outside one.
        let outside = match &ev {
            DbEvent::Failed { id: q, error: DbError::NeedsNoTransaction(_), .. } => self.tabs.get(id).and_then(|t| {
                let calls = |sql: &str| {
                    let first = datarig_core::sql::lexer::lex_in(sql, t.exec.prepared.language().dialect())
                        .into_iter()
                        .find(|w| w.is_word())
                        .map(|w| w.text(sql).to_ascii_uppercase());
                    matches!(first.as_deref(), Some("CALL" | "DO"))
                };
                let opened = t.exec.sent_manual && t.exec.tx_from.is_none() && !t.exec.path_per_transaction;
                let sql = t.exec.run.current().and_then(|i| t.exec.run.statements.get(i)).map(|s| s.sql.clone());
                sql.filter(|s| *q == t.exec.run_id && opened && calls(s))
            }),
            _ => None,
        };
        // A statement that cannot run in a transaction, refused only because the tab's schema
        // is set in one behind a pooler: said why, and how to run it.
        let needs_no_tx = (outside.is_none()
            && matches!(&ev, DbEvent::Failed { error: DbError::NeedsNoTransaction(_), .. }))
        .then(|| {
            let t = self.tabs.get(id);
            let own = profile.map(|p| self.own_database(p)).unwrap_or_default();
            t.and_then(|t| t.context.database.clone()).unwrap_or(own)
        });
        // Where the result's rows may spill, and how much.
        let (spill, limits) =
            (self.spill.clone(), self.tabs.get(id).map(|t| self.result_limits(t)).unwrap_or_default());
        // Fetching stops (the spill file is full or failed): the portal is closed after this.
        let mut stop: Option<Notice> = None;
        // A fetch of more rows failed or was cancelled.
        let mut fetch_failed = false;
        // The portal of this result to close after the event (a resumed statement's rows that
        // are not added, or stopped at the spill limit).
        let mut close_portal: Option<u64> = None;
        // The query of a table tab whose first page landed: kept in the filter history.
        let mut landed: Option<datarig_core::query::Query> = None;
        let page_size = self.page_size;
        let origin = self.tabs.get(id).and_then(Self::origin_now);
        let wall = self.wall_now();
        // A transaction opened for manual commit now keeps the rollback limit of this moment.
        let rollback = self.tabs.get(id).and_then(|t| self.tx_rollback_limit(t));
        let tx_keys = matches!(ev, DbEvent::ManualBegun { .. }).then(|| self.tx_keys());
        // The transaction the session opened for the run was rolled back: ask about running the
        // procedure outside one now.
        let mut offer = false;
        // The rollback the app sent for an idle transaction: confirmed (said in the status bar
        // too), or refused (`Err`: the session closes instead).
        let mut idle_end: Option<Result<Notice, (Duration, crate::app::idle::RollbackFailed)>> = None;
        // The plan format a result of the tab may be in (one shows as a plan).
        let explain = self.tab_explain(id);
        let Some(t) = self.tabs.get_mut(id) else { return };
        if outside.is_some() {
            t.exec.outside_offer = outside;
        }
        // A page with more rows keeps (or opens) the portal and restarts its idle count; any
        // other outcome of this session means no portal is open. A portal read inside the
        // user's own transaction is never closed for being idle.
        let in_block = t.exec.in_block;
        // A first page the driver did not hold (`DbEvent::Released` came before it): nothing is
        // open past it.
        let released = t.exec.released && matches!(ev, DbEvent::Page { columns: Some(_), .. });
        let paging = |more: bool| match (more, released) {
            (false, _) => Paging::None,
            (true, true) => Paging::Released,
            (true, false) => Paging::Open { since: now, in_block },
        };
        let connected = matches!(ev, DbEvent::Connected);
        let failed = matches!(ev, DbEvent::ConnectFailed { .. });
        let notice = match ev {
            DbEvent::Connected => {
                t.exec.state = SessionState::Ready;
                None
            }
            // Said by the profile's metadata session (a query session connects as it does).
            DbEvent::ReadOnlyPerTransaction | DbEvent::Encryption(_) => None,
            // Handled first (`statement_cache_off`, `sync_tab_language`, the cancel's clock).
            DbEvent::StatementCacheOff
            | DbEvent::Language(_)
            | DbEvent::Explain(_)
            | DbEvent::Edit(_)
            | DbEvent::SessionValues(_)
            | DbEvent::Stopping { .. }
            | DbEvent::CancelRefused { .. }
            | DbEvent::ValuesNotKnown { .. } => None,
            // A monitor session's own.
            DbEvent::Monitor(_)
            | DbEvent::MonitorSnapshot { .. }
            | DbEvent::MonitorQueryText { .. }
            | DbEvent::MonitorReplication { .. }
            | DbEvent::MonitorActionDone { .. }
            | DbEvent::MonitorFollowUp { .. }
            | DbEvent::ServerSession { .. } => None,
            // Statement `index` of the run opened a transaction for manual commit: the block that
            // follows is that one. Said in the run's Messages.
            DbEvent::ManualBegun { id: q, index } => {
                t.exec.manual_begun = Some((q, index));
                if q == t.exec.run_id {
                    let (commit, rollback) = tx_keys.unwrap_or_default();
                    let step = (index + 1).to_string();
                    t.exec.run.notes.push(Notice::new(Msg::TxBegan { step, commit, rollback }, Level::Info));
                }
                None
            }
            // The transaction opened for the run was rolled back at once (the run failed in it):
            // nothing it did is kept.
            DbEvent::ManualRolledBack { id: q, index } => {
                // Without a block reported open there is no `Block(false)` to come.
                t.exec.closing = t.exec.in_block.then_some(TxEnd::RolledBack);
                if q == t.exec.run_id {
                    let m = Notice::new(Msg::TxManualRolledBack { step: (index + 1).to_string() }, Level::Info);
                    t.exec.run.notes.push(m);
                    offer = t.exec.outside_offer.is_some();
                }
                None
            }
            // The failed run was undone to its savepoint: the transaction keeps what earlier runs
            // did, and nothing of this one.
            DbEvent::RunRolledBack { id: q } => {
                if let Some(tx) = t.exec.tx.as_mut() {
                    tx.undo_run(q);
                }
                if q == t.exec.run_id {
                    t.exec.run.notes.push(Notice::new(Label::TxRunRolledBack, Level::Info));
                    // A procedure that failed in the open transaction may be one that commits by
                    // itself (it cannot run in one): how to run it.
                    let dialect = t.exec.prepared.language().dialect();
                    let call = t
                        .exec
                        .run
                        .failed()
                        .and_then(|(i, e)| {
                            // PostgreSQL's 2D000, as its message says it (the driver's error
                            // carries no SQLSTATE).
                            e.contains("invalid transaction termination").then_some(i)
                        })
                        .and_then(|i| t.exec.run.statements.get(i))
                        .is_some_and(|s| {
                            let first = datarig_core::sql::lexer::lex_in(&s.sql, dialect)
                                .into_iter()
                                .find(|w| w.is_word())
                                .map(|w| w.text(&s.sql).to_ascii_uppercase());
                            matches!(first.as_deref(), Some("CALL" | "DO"))
                        });
                    if call {
                        t.exec.run.notes.push(Notice::new(Label::TxCallInTx, Level::Warning));
                    }
                }
                None
            }
            // The answer to the commit or rollback asked for.
            DbEvent::TxEnded { id: q, outcome } if t.exec.ending.is_some_and(|e| e.id == q) => {
                let ending = t.exec.ending.take();
                let commit = ending.is_some_and(|e| e.commit);
                let asked = if commit { TxOutcome::Committed } else { TxOutcome::RolledBack };
                t.exec.last_end = Some(outcome == asked);
                t.exec.closing =
                    Some(if outcome == TxOutcome::Committed { TxEnd::Committed } else { TxEnd::RolledBack });
                let (count, rows, began, began_wall) =
                    t.exec.tx.as_ref().map_or((0, 0, now, wall), |x| (x.changes, x.rows, x.began, x.began_wall));
                let open = wall.duration_since(began_wall).unwrap_or_else(|_| now.saturating_duration_since(began));
                let time = crate::app::tx::fmt_clock(open);
                let idle = ending.and_then(|e| e.idle);
                let m = match outcome {
                    // The app's rollback of an idle transaction: the session stays.
                    _ if idle.is_some() => {
                        let time = format_timeout(idle.map(|i| i.limit));
                        Notice::new(Msg::TxIdleRolledBack { count, time }, Level::Warning)
                    }
                    TxOutcome::Committed => {
                        let rows = self.i18n.msg(&Msg::TxRows { count: rows }).to_string();
                        Notice::new(Msg::TxCommitted { count, rows, time }, Level::Success)
                    }
                    TxOutcome::RolledBack => Notice::new(Msg::TxRolledBack { count, time }, Level::Success),
                    TxOutcome::RolledBackInstead => Notice::new(Label::TxRolledBackInstead, Level::Warning),
                };
                t.exec.run.notes.push(m.clone());
                if idle.is_some() {
                    idle_end = Some(Ok(m.clone()));
                }
                Some(m)
            }
            DbEvent::TxEnded { .. } => None,
            // The answer to the Apply of a result's changes.
            DbEvent::Applied { id: qid, result } => return self.edit_applied(id, qid, result),
            // The app's rollback of an idle transaction failed: the session closes instead (after
            // this event), whatever the error.
            DbEvent::Failed { id: q, error, .. } if t.exec.ending.is_some_and(|e| e.id == q && e.idle.is_some()) => {
                let limit = t.exec.ending.take().and_then(|e| e.idle).map_or(Duration::ZERO, |i| i.limit);
                t.exec.last_end = Some(false);
                // The server's own idle limit ended the session first: said as such.
                let why = match error {
                    DbError::IdleInTransactionTimeout(_) => crate::app::idle::RollbackFailed::ServerEnded(text.clone()),
                    _ => crate::app::idle::RollbackFailed::Refused(text.clone()),
                };
                idle_end = Some(Err((limit, why)));
                None
            }
            // The commit or rollback asked for failed.
            DbEvent::Failed { id: q, error, .. } if t.exec.ending.is_some_and(|e| e.id == q) => {
                let commit = t.exec.ending.take().is_some_and(|e| e.commit);
                t.exec.last_end = Some(false);
                let m = match error {
                    DbError::NoTransaction => Notice::new(Label::TxNone, Level::Warning),
                    // The connection ended while committing: the transaction may have been
                    // committed. It is no longer followed, so the `Lost` that comes says nothing
                    // of a rollback (nor does the next run): only that the outcome is unknown.
                    DbError::CommitUnknown => {
                        t.exec.tx_open = false;
                        t.block_ended(TxEnd::Unknown);
                        Notice::new(Label::DbCommitUnknown, Level::Error)
                    }
                    DbError::NotSupported => Notice::new(Label::TxNotSupported, Level::Warning),
                    // The server refused the commit (a deferred constraint, a serialization
                    // failure): it rolled the transaction back.
                    _ if commit => {
                        t.exec.closing = Some(TxEnd::RolledBack);
                        Notice::new(Msg::TxCommitFailed { error: text.clone() }, Level::Error)
                    }
                    _ => Notice::new(Msg::TxRollbackFailed { error: text.clone() }, Level::Error),
                };
                t.exec.run.notes.push(m.clone());
                Some(m)
            }
            // The server ended the open transaction by itself during the run (MySQL: a deadlock,
            // an implicit commit, a procedure that failed).
            DbEvent::TxClosedBy { how, .. } => {
                // During an export (which reads, and is no run's statement) the run's log is
                // booked already.
                if t.exec.running.is_none_or(|r| !r.export) {
                    t.book_tx();
                }
                let count = t.exec.tx.as_ref().map_or(0, |x| x.changes);
                let step = (t.exec.run.current().or_else(|| t.ending_statement()).unwrap_or(0) + 1).to_string();
                let (m, end) = match how {
                    ServerEnd::RolledBack => {
                        (Notice::new(Msg::TxServerRolledBack { count }, Level::Warning), TxEnd::RolledBack)
                    }
                    ServerEnd::Committed => {
                        (Notice::new(Msg::TxCommittedImplicitly { step, count }, Level::Warning), TxEnd::Committed)
                    }
                    ServerEnd::Unknown => (Notice::new(Label::TxClosedUnknown, Level::Warning), TxEnd::Unknown),
                };
                t.exec.closing = t.exec.in_block.then_some(end);
                // A commit or rollback the server refused says so itself.
                if t.exec.ending.is_none() {
                    t.exec.run.notes.push(m);
                }
                None
            }
            DbEvent::Autocommit(on) => {
                t.exec.autocommit_off = !on;
                None
            }
            DbEvent::Isolation(level) => {
                t.exec.isolation = level;
                None
            }
            // The server dropped the schema's startup option (a pooler): the driver sets the
            // path in each transaction instead. Said in the run's Messages.
            DbEvent::SessionPerTransaction { .. } => {
                t.exec.session_pooled = true;
                if let Some(m) = &per_transaction {
                    t.exec.run.notes.push(m.clone());
                }
                per_transaction.clone()
            }
            DbEvent::ContextPerTransaction => {
                let schema = t.context.schema.clone().unwrap_or_default();
                let m = Notice::new(Msg::ContextPerTransaction { schema }, Level::Info);
                t.exec.run.notes.push(m.clone());
                t.exec.path_per_transaction = true;
                // The run that opened the session was sent before this was known.
                if t.exec.running.is_some() {
                    t.warn_session_path();
                }
                Some(m)
            }
            // While the profile connects, its metadata session reports the same failure.
            DbEvent::ConnectFailed { .. } if connecting => None,
            DbEvent::ConnectFailed { .. } => {
                if let Some(s) = t.exec.session.take() {
                    s.close();
                }
                t.exec.state = SessionState::Idle;
                t.exec.running = None;
                // A table tab's query whose run waited for this session is not applied.
                if let Some(tq) = t.doc.table_query.as_mut() {
                    tq.run_over(t.exec.query_id, crate::app::table_query::Unapplied::Ended);
                }
                t.exec.unsure();
                t.exec.paging = Paging::None;
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = Results::Error(text.clone());
                }
                t.exec.replace_pending = false;
                t.exec.view = crate::app::tabs::ResultView::Rows;
                Some(Notice::new(Msg::ConnFailed { error: text }, Level::Error))
            }
            DbEvent::Schemas(_)
            | DbEvent::Objects { .. }
            | DbEvent::Catalog(_)
            | DbEvent::Keys(_)
            | DbEvent::Structure { .. }
            | DbEvent::Ddl { .. }
            | DbEvent::TableKeys { .. }
            | DbEvent::Databases(_) => None,
            // Where the session works, as the server says: a chosen schema it does not list
            // does not exist there or cannot be used (said, never taken as fine).
            // The run's Messages keep it (a status alone is replaced by the run's outcome at
            // once), and the editor's first line marks the schema.
            DbEvent::Context { database, schemas } => {
                let missing = t.context.schema.clone().filter(|s| !schemas.contains(s));
                t.exec.context = Some((database.clone(), schemas));
                let m =
                    missing.map(|schema| Notice::new(Msg::ContextSchemaMissing { schema, database }, Level::Warning));
                if let Some(m) = &m {
                    t.exec.run.notes.push(m.clone());
                }
                m
            }
            DbEvent::TxOpen(open) => {
                t.exec.tx_open = open;
                t.exec.tx_aborted &= open;
                None
            }
            DbEvent::TxAborted(aborted) => {
                t.exec.tx_aborted = aborted;
                None
            }
            // The coming first page of the running query is not held: past it the statement can
            // only run again.
            DbEvent::Released { id: qid } => {
                if qid == t.exec.query_id {
                    t.exec.released = true;
                }
                None
            }
            // The user's own transaction: results read in it are labelled,
            // and say how it ended once it did. `Block` comes before the `TxOpen` of the same
            // change, so an aborted block is still known here.
            DbEvent::Block(true) => {
                let manual = t.exec.manual_begun.take();
                t.block_began(manual.is_some(), now, wall, rollback);
                // Which statements of the last run ran in it: from the one that opened it on.
                let query = t.exec.run_id;
                match manual {
                    Some((q, index)) if q == query => {
                        t.exec.tx_from = Some(index);
                        // Reported after the run's answer: what it did goes in now.
                        if t.exec.running.is_none() {
                            t.book_tx();
                        }
                    }
                    Some(_) => t.exec.tx_from = None,
                    None => t.exec.tx_from = t.exec.running.and_then(|_| t.exec.run.current()),
                }
                None
            }
            DbEvent::Block(false) => {
                let end = match t.exec.closing.take() {
                    Some(end) => end,
                    None => {
                        let end = t.ending_outcome();
                        if let Some(i) = t.ending_statement().filter(|_| end == TxEnd::RolledBack) {
                            t.exec.run.rolled_back(i);
                        }
                        end
                    }
                };
                t.block_ended(end);
                None
            }
            // A count the user asked for, or one of the preview of a result's changes.
            DbEvent::Counted { id: qid, result, snapshot } => {
                if self.edit_preview_counted(id, qid, &result) {
                    return;
                }
                return self.on_counted(id, qid, result, snapshot);
            }
            // A count of a column's values the filter dialog asked for.
            DbEvent::GroupCounted { id: qid, result, .. } => return self.on_group_counted(id, qid, result),
            // The planner's estimate the user asked for.
            DbEvent::Estimated { id: qid, result } => return self.on_estimated(id, qid, result),
            // A statement of an export.
            DbEvent::Exported { id: qid, result, cancelled } => return self.on_exported(id, qid, result, cancelled),
            // The server's half of the allowlist, for a text plan to view as a plan.
            DbEvent::RepeatChecked { id: check, result } => return self.as_plan_checked(id, check, result),
            DbEvent::Lost { error } => {
                t.exec.session = None;
                t.exec.unsure();
                t.exec.state = SessionState::Lost;
                t.exec.lost_tx = t.exec.tx_at_risk();
                t.exec.running = None;
                // A table tab's query whose run had not brought its rows is not applied.
                if let Some(tq) = t.doc.table_query.as_mut() {
                    tq.run_over(t.exec.query_id, crate::app::table_query::Unapplied::Ended);
                }
                let had_tx = t.exec.user_tx();
                // Lost while the app rolled its idle transaction back: said in the status bar
                // too, as a failed rollback is.
                let idle = t.exec.ending.is_some_and(|e| e.idle.is_some());
                t.tx_reset();
                t.exec.paging = Paging::None;
                if let DbError::IdleInTransactionTimeout(_) = error {
                    let m = Notice::new(Msg::TxIdleTimeout { error: text }, Level::Warning);
                    t.exec.run.notes.push(m.clone());
                    if idle {
                        idle_end = Some(Ok(m.clone()));
                    }
                    Some(m)
                } else if let DbError::IdleSessionTimeout(_) = error {
                    // The server's idle limit, not a failure: the next run connects again (and
                    // does not say again that a transaction went).
                    t.exec.lost_tx = false;
                    let m = match had_tx {
                        true => Notice::new(Msg::QueryIdleClosedByServerTx { error: text }, Level::Warning),
                        false => Notice::new(Msg::QueryIdleClosedByServer { error: text }, Level::Info),
                    };
                    t.exec.run.notes.push(m.clone());
                    Some(m)
                } else {
                    if had_tx {
                        t.exec.run.notes.push(Notice::new(Label::TxLost, Level::Warning));
                    }
                    let lost = Notice::new(Msg::QueryLost { error: text }, Level::Warning);
                    if idle {
                        idle_end = Some(Ok(lost.clone()));
                    }
                    // Lost while committing: what matters is that the outcome is not known.
                    let unknown = Msg::Label(Label::DbCommitUnknown);
                    match t.exec.run.notes.last().filter(|n| n.msg == unknown).cloned() {
                        Some(n) => {
                            t.exec.run.notes.push(lost);
                            Some(n)
                        }
                        None => Some(lost),
                    }
                }
            }
            DbEvent::Page { id: qid, .. }
            | DbEvent::Done { id: qid, .. }
            | DbEvent::Failed { id: qid, .. }
            | DbEvent::Started { id: qid, .. }
            | DbEvent::StepRows { id: qid, .. }
            | DbEvent::Finished { id: qid, .. }
            | DbEvent::Info { id: qid, .. }
                if qid != t.exec.query_id =>
            {
                None
            }
            // What the server said of a statement beyond its outcome: in the run's Messages.
            DbEvent::Info { index, info, .. } => {
                let several = t.exec.run.several();
                for m in info_notices(&info) {
                    let m = if several {
                        let text = self.i18n.msg(&m.msg).to_string();
                        Notice::new(Msg::QueryInfoStep { step: (index + 1).to_string(), info: text }, m.level)
                    } else {
                        m
                    };
                    t.exec.run.notes.push(m);
                }
                None
            }
            // A run of several statements: which one runs, and what each before the last did.
            DbEvent::Started { index, .. } => {
                t.exec.run.started(index);
                let query = t.exec.query_id;
                t.exec.reading = crate::app::tabs::Reading { query, step: Some(index), ..Default::default() };
                None
            }
            // The rows of a statement before the last: kept as its own result, up to its limit
            // (past it the statement still runs to its end; the rest of its rows are dropped).
            DbEvent::StepRows { index, columns, rows, more, .. } => {
                if columns.is_some() && t.exec.replace_pending {
                    t.drop_rows();
                }
                let count = match (columns, t.exec.steps.get_mut(&index)) {
                    (Some(cols), _) => {
                        if let Some((plan, source, format)) = crate::app::plan::plan_of(&cols, &rows, more, explain) {
                            t.exec.plan =
                                Some(crate::app::plan::PlanTab::new(plan, t.exec.query_id, index, &source, format));
                        }
                        let store = datarig_core::results::RowStore::new(rows, spill, limits);
                        let mut rs = ResultSet::new(cols, store, more, &null);
                        rs.read_in = read_in(&mut t.exec);
                        rs.edit.bind(bound);
                        let n = rs.rows.len();
                        let grid = GridState::paged(page_size, t.grid.page_rows);
                        t.exec.steps.insert(index, crate::app::tabs::Parked { rs, grid });
                        n
                    }
                    (None, Some(p)) => {
                        let rs = &mut p.rs;
                        rs.read_in.page(t.exec.values.as_ref());
                        if rs.more {
                            let read = rs.rows.len();
                            rs.more = match rs.rows.append(rows) {
                                Ok(a) => more && !a.capped,
                                Err(_) => false,
                            };
                            p.grid.rows_came(read, rs.rows.len() - read);
                        }
                        rs.rows.len()
                    }
                    (None, None) => 0,
                };
                t.exec.run.step_rows(index, count as u64, more);
                None
            }
            DbEvent::Finished { index, outcome, elapsed, .. } => {
                t.exec.reading = crate::app::tabs::Reading { query: t.exec.query_id, ..Default::default() };
                t.exec.succeeded(Some(index));
                t.exec.run.finished(index, &outcome, elapsed);
                t.statement_done(index);
                None
            }
            ev @ (DbEvent::Page { .. } | DbEvent::Done { .. } | DbEvent::Failed { .. })
                if self.transient.as_ref().is_some_and(|(m, _)| {
                    matches!(m.msg, Msg::QueryBusy { .. } | Msg::Label(Label::QueryNoStatement))
                }) =>
            {
                // A finished query makes a busy/no-statement flash obsolete; show the result instead.
                self.transient = None;
                return self.tab_event(id, ev);
            }
            // The statement ran again past its closed portal: its next page
            // goes after the rows the result has, when its columns are the same.
            DbEvent::Page { columns: Some(cols), mut rows, mut more, elapsed, .. } if t.exec.resuming.is_some() => {
                let r = t.exec.resuming.take();
                t.exec.running = None;
                t.exec.released = false;
                let same = r.as_ref().is_some_and(|r| {
                    r.columns.len() == cols.len()
                        && r.columns.iter().zip(&cols).all(|((n, ty), c)| *n == c.name && *ty == c.type_name)
                });
                // What was announced when it was sent stays in the status bar with its rows.
                let announced = t.exec.run.notes.last().cloned();
                let values = t.exec.values.clone();
                let cap = shown_cap(t);
                let qid = t.exec.query_id;
                let answer = t.answer_rows_mut();
                match (answer, r) {
                    (Some((rs, grid)), Some(r)) if same => {
                        rs.read_in.page(values.as_ref());
                        let read = rs.rows.len();
                        // A table tab's cap: the rows past it are left out, and nothing is
                        // read past it.
                        if let Some(open) = crate::app::table_query::cap_page(&mut rows, read, cap, more) {
                            more = false;
                            if open && !released {
                                close_portal = Some(qid);
                            }
                        }
                        let appended = rs.rows.append(rows);
                        grid.rows_came(read, rs.rows.len() - read);
                        match appended {
                            Ok(a) if a.capped => {
                                let count = rs.rows.len() as u64;
                                let limit = fmt_bytes(limits.cap.unwrap_or(0));
                                stop = Some(Notice::new(Msg::ResultsSpillCapped { count, limit }, Level::Warning));
                            }
                            Ok(_) => rs.more = more,
                            Err(fault) => {
                                let error = self.i18n.msg(&persist::fault_reason(&fault)).to_string();
                                stop = Some(Notice::new(Msg::ResultsSpillFailed { error }, Level::Error));
                            }
                        }
                        if stop.is_some() {
                            rs.more = false;
                        }
                        if (r.page * page_size) < rs.rows.len() {
                            grid.set_page(r.page);
                        }
                        let (n, more) = (rs.rows.len(), rs.more);
                        t.exec.paging = if stop.is_some() { Paging::Stopped } else { paging(more) };
                        if stop.is_some() {
                            close_portal = Some(t.exec.query_id);
                        }
                        stop.take().or(announced).or(Some(rows_msg(n, more, elapsed)))
                    }
                    _ => {
                        // Not the result it was: nothing is added, and its portal goes (one
                        // not held is gone already).
                        if more {
                            t.exec.paging = Paging::None;
                            if !released {
                                close_portal = Some(t.exec.query_id);
                            }
                        }
                        Some(Notice::new(Label::ResultsPageColumnsChanged, Level::Warning))
                    }
                }
            }
            // It ended without rows: nothing is added; the portal stays closed.
            DbEvent::Done { .. } | DbEvent::Failed { .. } if t.exec.resuming.is_some() => {
                t.exec.resuming = None;
                t.exec.running = None;
                t.exec.released = false;
                // Refused: the next page no longer offers to run it again.
                t.exec.rerun_ok &= refused.is_none();
                let m = if let Some(m) = refused {
                    m
                } else if text.is_empty() {
                    Notice::new(Label::ResultsPageColumnsChanged, Level::Warning)
                } else {
                    Notice::new(Msg::QueryError { error: text.clone() }, Level::Error)
                };
                t.exec.run.notes.push(m.clone());
                Some(m)
            }
            DbEvent::Page { columns: Some(cols), mut rows, mut more, elapsed, .. } => {
                t.exec.succeeded(None);
                t.exec.running = None;
                t.exec.released = false;
                // A table tab's cap on its query's rows: the first page past it is cut there,
                // and what the server holds open past it is closed (one not held is gone).
                let query = t.exec.query_id;
                let cap = t
                    .doc
                    .table_query
                    .as_ref()
                    .and_then(|tq| tq.requested.as_ref())
                    .filter(|(q, _)| *q == query)
                    .and_then(|(_, q)| q.limit);
                if let Some(open) = crate::app::table_query::cap_page(&mut rows, 0, cap, more) {
                    more = false;
                    if open && !released {
                        close_portal = Some(query);
                    }
                }
                let count = rows.len();
                // A table tab's rows read again with the same columns (a new filter or order):
                // the cursor stays on its column, the view where it was sideways, so that the
                // sort key pressed again turns the same column.
                let kept_col = match &t.results {
                    Results::Rows(old)
                        if t.is_table()
                            && old.columns.len() == cols.len()
                            && old.columns.iter().zip(&cols).all(|(o, c)| o.meta.name == c.name) =>
                    {
                        Some((t.grid.col, t.grid.x_off))
                    }
                    _ => None,
                };
                // A plan (`EXPLAIN (FORMAT JSON)`, MySQL's tree): shown as one; its rows stay a
                // result tab.
                let plan = if t.is_table() { None } else { crate::app::plan::plan_of(&cols, &rows, more, explain) };
                // The server said it has no plan of the format for the statement: said, not just
                // left as rows.
                let no_plan = plan.is_none() && !t.is_table() && crate::app::plan::says_no_plan(&cols, &rows, explain);
                let store = datarig_core::results::RowStore::new(rows, spill, limits);
                // The run's answer: its own result tab, shown.
                if t.exec.replace_pending {
                    t.drop_rows();
                }
                t.exec.paging = paging(more);
                t.park_shown();
                let page_rows = t.grid.page_rows;
                let mut rs = ResultSet::new(cols, store, more, &null);
                // Bound to the session's values now: a later `SET time_zone` is not this result's.
                rs.read_in = read_in(&mut t.exec);
                rs.edit.bind(bound);
                let query = t.exec.query_id;
                if t.exec.reading.query != query {
                    t.exec.reading = crate::app::tabs::Reading { query, ..Default::default() };
                }
                t.exec.reading.answer = true;
                t.results = Results::Rows(Box::new(rs));
                t.grid = GridState::paged(page_size, page_rows);
                if let Some((col, x_off)) = kept_col {
                    (t.grid.col, t.grid.x_off) = (col, x_off);
                }
                t.exec.origin = origin;
                t.exec.shown = t.exec.run.len().checked_sub(1);
                // A table tab's rows are of the query its run sent, if it sent one.
                if let Some(tq) = t.doc.table_query.as_mut() {
                    landed = tq.first_page(query);
                }
                // The Chart tab shown before stays: it draws these rows.
                let charted = t.exec.view == crate::app::tabs::ResultView::Chart;
                t.exec.view =
                    if charted { crate::app::tabs::ResultView::Chart } else { crate::app::tabs::ResultView::Rows };
                if let Some((plan, source, format)) = plan {
                    let (query, index) = (t.exec.query_id, t.exec.shown.unwrap_or(0));
                    t.exec.plan = Some(crate::app::plan::PlanTab::new(plan, query, index, &source, format));
                    t.exec.view = crate::app::tabs::ResultView::Plan;
                }
                t.exec.run.answered(StatementOutcome::Rows { count: count as u64, more }, Some(elapsed));
                // Not held: the next page runs the statement again when the allowlist lets it;
                // otherwise the first page is all there is, and the user learns why and what to do.
                let refusal = if released && more { t.exec.prepared.repeatable(t.answer_sql()).err() } else { None };
                if released && more {
                    t.exec.rerun_ok = refusal.is_none();
                }
                if let Some(r) = refusal {
                    let m = Notice::new(
                        crate::app::pages::first_page_only(
                            &self.i18n,
                            t.exec.prepared.language(),
                            t.answer_sql(),
                            &r,
                            count as u64,
                        ),
                        Level::Warning,
                    );
                    t.exec.run.notes.push(m.clone());
                    t.exec.explain_rolled_back = false;
                    Some(m)
                } else if no_plan {
                    if std::mem::take(&mut t.exec.explain_rolled_back) {
                        t.exec.run.notes.push(Notice::new(Label::SafetyExplainRolledBack, Level::Info));
                    }
                    let m = Notice::new(Label::PlanNoTree, Level::Info);
                    t.exec.run.notes.push(m.clone());
                    Some(m)
                } else {
                    Some(if std::mem::take(&mut t.exec.explain_rolled_back) {
                        Notice::new(Label::SafetyExplainRolledBack, Level::Info)
                    } else {
                        // The warning would stay behind the grid, in Messages.
                        t.session_path_note().unwrap_or_else(|| rows_msg(count, more, elapsed))
                    })
                }
            }
            DbEvent::Page { columns: None, mut rows, mut more, elapsed, .. } => {
                t.exec.running = None;
                // A table tab's cap: the rows past it are left out, and its portal is closed.
                let cap = shown_cap(t);
                let held = t.answer_rows_mut().map_or(0, |(rs, _)| rs.rows.len());
                if let Some(open) = crate::app::table_query::cap_page(&mut rows, held, cap, more) {
                    more = false;
                    if open {
                        close_portal = Some(t.exec.query_id);
                    }
                }
                t.exec.paging = paging(more);
                let want = t.exec.want_page.take();
                let values = t.exec.values.clone();
                let answer = t.answer_rows_mut();
                let m = match answer {
                    Some((rs, grid)) => {
                        rs.read_in.page(values.as_ref());
                        let read = rs.rows.len();
                        let appended = rs.rows.append(rows);
                        grid.rows_came(read, rs.rows.len() - read);
                        match appended {
                            Ok(a) if a.capped => {
                                let count = rs.rows.len() as u64;
                                let limit = fmt_bytes(limits.cap.unwrap_or(0));
                                stop = Some(Notice::new(Msg::ResultsSpillCapped { count, limit }, Level::Warning));
                            }
                            Ok(_) => rs.more = more,
                            Err(fault) => {
                                let log = ErrorLog::new(self.paths.errors_log());
                                log.record("results.spill_failed", &fault);
                                let error = self.i18n.msg(&persist::fault_reason(&fault)).to_string();
                                stop = Some(Notice::new(Msg::ResultsSpillFailed { error }, Level::Error));
                            }
                        }
                        if stop.is_some() {
                            rs.more = false;
                        }
                        let (n, more) = (rs.rows.len(), rs.more);
                        // The page asked for, once its rows are there.
                        if let Some(p) = want.filter(|p| p * page_size < n) {
                            grid.set_page(p);
                        }
                        if stop.is_some() {
                            t.exec.paging = Paging::Stopped;
                        }
                        t.exec.run.rows_fetched(n as u64, more);
                        Some(rows_msg(n, more, elapsed))
                    }
                    None => None,
                };
                if let Some(m) = m {
                    self.tab_status(id, m);
                }
                if let Some(n) = stop {
                    let qid = self.tabs.get(id).map_or(0, |t| t.exec.query_id);
                    self.send_tab(id, DbCommand::ClosePortal { id: qid });
                    self.tab_status(id, n);
                    // A copy of every row takes the rows fetched (and says so).
                    self.fetch_copy_page(id);
                    return;
                }
                self.fetch_copy_page(id);
                None
            }
            DbEvent::Done { outcome, elapsed, .. } => {
                t.exec.succeeded(None);
                t.exec.running = None;
                t.exec.released = false;
                // Rows an earlier run left on screen keep their paging state (closed).
                if t.exec.kept_log.is_none() {
                    t.exec.paging = Paging::None;
                }
                let done = match &outcome {
                    Outcome::Affected(n) => StatementOutcome::Affected(*n),
                    Outcome::Command(tag) => StatementOutcome::Command(tag.clone()),
                };
                if let Some(i) = t.exec.run.answered(done, Some(elapsed)) {
                    t.statement_done(i);
                }
                let m = Notice::new(
                    match outcome {
                        Outcome::Affected(count) => Msg::QueryDoneAffected { count, elapsed },
                        Outcome::Command(tag) => Msg::QueryDoneCommand { tag, elapsed },
                    },
                    Level::Success,
                );
                // Row results stay where they are (this run's, or an earlier run's when this
                // one returned none); the last result tab of this run is shown, else Messages.
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = Results::Message(m.clone());
                }
                t.run_ended(false);
                Some(if std::mem::take(&mut t.exec.explain_rolled_back) {
                    Notice::new(Label::SafetyExplainRolledBack, Level::Info)
                } else {
                    t.session_path_note().unwrap_or(m)
                })
            }
            DbEvent::Failed { cancelled, .. } => {
                // The failed statement and the ones after it (never run) are unknown.
                t.exec.unsure();
                t.exec.released = false;
                let was_fetch = t.exec.running.is_some_and(|r| r.fetch);
                fetch_failed = was_fetch;
                t.exec.running = None;
                if was_fetch {
                    // The portal is gone, the rows fetched stay, and whether the server has
                    // more is unknown (not absent): the next page goes the way of a closed
                    // portal (run again when the allowlist lets it, else refused), and the
                    // result never claims to be complete.
                    t.exec.paging = Paging::Interrupted;
                    t.exec.rerun_ok = t.exec.prepared.repeatable(t.answer_sql()).is_ok();
                } else if t.exec.kept_log.is_none() {
                    // Rows an earlier run left on screen keep their paging state (closed).
                    t.exec.paging = Paging::None;
                }
                // A run of several statements cancelled between two of them: no statement was
                // running, the next one never started.
                let between =
                    (!was_fetch && cancelled && t.exec.run.several()).then(|| t.exec.run.cancelled_between()).flatten();
                // Which statement of the run it was (a failed fetch is the last one's rows).
                let step = if was_fetch || between.is_some() {
                    None
                } else {
                    let outcome =
                        if cancelled { StatementOutcome::Cancelled } else { StatementOutcome::Failed(text.clone()) };
                    t.exec.run.answered(outcome, None).filter(|_| t.exec.run.several())
                };
                let total = t.exec.run.len();
                let m = match (between, step) {
                    (Some(i), _) if i + 1 == total => {
                        let sql = crate::app::runlog::excerpt(&t.exec.run.statements[i].sql, 40);
                        let (step, total) = ((i + 1).to_string(), total.to_string());
                        Notice::new(Msg::QueryCancelledBeforeLast { step, total, sql }, Level::Warning)
                    }
                    (Some(i), _) => {
                        let (from, to, total) = ((i + 1).to_string(), total.to_string(), total.to_string());
                        Notice::new(Msg::QueryCancelledBefore { from, to, total }, Level::Warning)
                    }
                    (None, Some(i)) => {
                        let (step, total) = ((i + 1).to_string(), t.exec.run.len().to_string());
                        let sql = crate::app::runlog::excerpt(&t.exec.run.statements[i].sql, 40);
                        if cancelled {
                            Notice::new(Msg::QueryStepCancelled { step, total, sql }, Level::Warning)
                        } else {
                            Notice::new(Msg::QueryStepFailed { step, total, sql, error: text.clone() }, Level::Error)
                        }
                    }
                    (None, None) if cancelled => Notice::new(Label::QueryCancelled, Level::Warning),
                    (None, None) => match needs_no_tx.clone() {
                        Some(database) => {
                            let error = text.clone();
                            Notice::new(Msg::ContextNoTransactionError { error, database }, Level::Error)
                        }
                        None => Notice::new(Msg::QueryError { error: text.clone() }, Level::Error),
                    },
                };
                if let Some(database) = needs_no_tx {
                    t.exec.run.notes.push(Notice::new(Msg::ContextNoTransaction { database }, Level::Warning));
                }
                // Row results stay (a failed fetch keeps `more` as it was); Messages says what
                // failed.
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = if cancelled { Results::Cancelled } else { Results::Error(text) };
                }
                if !was_fetch {
                    t.run_ended(true);
                }
                Some(m)
            }
        };
        if let Some(m) = notice {
            self.tab_status(id, m);
        }
        // The server answered the tab's query with its rows: it ran.
        if let Some(q) = landed {
            self.filter_history_record(id, &q);
        }
        // The rows of a table tab's query: its columns' values are remembered.
        if rows_for == Some(None) {
            self.remember_values(id);
            self.filter_dialog_columns_came(id);
        }
        match idle_end {
            // Said where the user looks, from another tab too.
            Some(Ok(m)) => self.status = Some(m),
            Some(Err((limit, why))) => return self.idle_rollback_failed(id, limit, why),
            None => {}
        }
        if let Some(qid) = close_portal {
            self.send_tab(id, DbCommand::ClosePortal { id: qid });
        }
        if let (Some(target), Some(t)) = (rows_for, self.tabs.get_mut(id)) {
            let (in_block, epoch) = (t.exec.in_block, t.exec.tx_epoch);
            let rs = match target {
                None => t.answer_rows_mut().map(|(rs, _)| rs),
                Some(i) => t.rows_of_mut(i).map(|(rs, _)| rs),
            };
            if let Some(rs) = rs {
                rs.mark_read(in_block, epoch);
            }
        }
        if fetch_failed {
            self.fetch_copy_failed(id);
        }
        // What the run did in the open transaction goes in its log; its idle time starts over.
        if let Some(t) = self.tabs.get_mut(id) {
            if run_ended {
                t.book_tx();
            }
            if (run_ended || rows_for.is_some() || t.exec.running.is_some())
                && let Some(open) = t.exec.tx.as_mut()
            {
                open.touch(now, wall);
            }
        }
        if offer {
            self.offer_outside(id);
        }
        self.end_answered(id);
        // The summary of a transaction that ended closes.
        if self.overlays.tx_summary().is_some_and(|s| s.tab == id)
            && !self.tabs.get(id).is_some_and(|t| t.exec.user_tx())
        {
            self.overlays.close(OverlayKind::TxSummary);
        }
        if rows_for == Some(None) {
            self.read_held_on(id);
        }
        self.keys_after_ddl(id, profile, run_ended, tx_ended);
        if connected {
            self.table_session_ready(id);
        }
        if failed {
            self.table_session_failed(id);
        }
    }

    /// Tab `id`'s result is held open on a session whose statement runs on until the result is
    /// read to its end (`Capabilities::reads_held_to_end`, MySQL): its next page is asked for at
    /// once, so the statement ends (and its locks go) as soon as the rows are in the result's
    /// store, not when the user pages on.
    fn read_held_on(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id) else { return };
        let reads = t.exec.session.as_ref().is_some_and(|s| s.caps.reads_held_to_end);
        if reads
            && t.exec.running.is_none()
            && t.exec.resuming.is_none()
            && matches!(t.exec.paging, Paging::Open { .. })
        {
            self.fetch_page(id);
        }
    }

    /// After DDL run in tab `id` the key cache of its profile, and the keys its table tabs read
    /// for their order, may be out of date: read again once the run ends, or, inside a
    /// transaction (the metadata session cannot see its changes yet), once the transaction ends.
    fn keys_after_ddl(&mut self, id: TabId, profile: Option<ProfileId>, run_ended: bool, tx_ended: bool) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let mut reload = false;
        if run_ended && std::mem::take(&mut t.exec.ddl) {
            if t.exec.tx_open {
                t.exec.ddl_in_tx = true;
            } else {
                reload = true;
            }
        }
        if tx_ended && std::mem::take(&mut t.exec.ddl_in_tx) {
            reload = true;
        }
        if reload && let Some(p) = profile {
            self.reload_keys(p);
            self.forget_order_keys(p, None);
            self.conns.forget_checks(p);
        }
    }
}

/// A size as the settings write it: `1 GB`, `512 MB`, `64 KB`, `1000 B`.
pub(crate) fn fmt_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 && n.is_multiple_of(1 << 30) => format!("{} GB", n >> 30),
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 && n.is_multiple_of(1 << 20) => format!("{} MB", n >> 20),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 && n.is_multiple_of(1 << 10) => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

/// What the server said of a statement beyond its outcome ([`StatementInfo`]), as notices of the
/// run's Messages: warnings as such, the rest as information.
fn info_notices(info: &datarig_core::driver::StatementInfo) -> Vec<Notice> {
    let mut out = Vec::new();
    if let Some(id) = info.insert_id {
        out.push(Notice::new(Msg::QueryInfoInsertId { id: id.to_string() }, Level::Info));
    }
    if info.warnings > 0 {
        out.push(Notice::new(Msg::QueryInfoWarnings { count: u64::from(info.warnings) }, Level::Warning));
    }
    if let Some(text) = &info.message {
        out.push(Notice::new(Msg::QueryInfoServer { text: text.clone() }, Level::Info));
    }
    if info.more_results > 0 {
        out.push(Notice::new(Msg::QueryInfoMoreResults { count: u64::from(info.more_results) }, Level::Warning));
    }
    if !info.not_undone.is_empty() {
        out.push(Notice::new(Msg::QueryInfoNotUndone { tables: info.not_undone.join(", ") }, Level::Warning));
    }
    out
}

fn rows_msg(count: usize, more: bool, elapsed: Duration) -> Notice {
    let count = count as u64;
    let m = if more { Msg::QueryMoreRows { count, elapsed } } else { Msg::QueryDoneRows { count, elapsed } };
    Notice::new(m, Level::Success)
}

/// What a result that starts now in session `exec` is read in: not known when its session said
/// server code runs (`DbEvent::ValuesNotKnown`), changed when the values changed while its
/// statement ran (`DbEvent::SessionValues` before its first page), else the session's values
/// now. What the session said is for this result only.
fn read_in(exec: &mut crate::app::tabs::TabSession) -> ReadIn {
    let reading = &mut exec.reading;
    let this = reading.query == exec.query_id;
    let read_in = if this && reading.not_known {
        ReadIn::Unknown
    } else if this && reading.moved {
        ReadIn::Changed
    } else {
        ReadIn::first(exec.values.as_ref())
    };
    reading.not_known = false;
    read_in
}

/// The cap on the rows of table tab `t`'s shown query, when it has one: the pages read after
/// the first stop there.
fn shown_cap(t: &crate::app::Tab) -> Option<u64> {
    t.doc.table_query.as_ref()?.shown.as_ref()?.limit
}
