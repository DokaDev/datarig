//! Workspace tabs: each tab has its own editor, results and
//! query session. [`TabManager`] keeps them in order, knows the active one and hands out ids.

use super::{Focus, Notice, Paging, Popup, Results, Running};
use crate::widgets::editor::Editor;
use crate::widgets::grid::GridState;
use datarig_core::driver::{Session, SessionContext};
use datarig_core::profile::ProfileId;
use datarig_core::scripts::Stamp;
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::risk::Classifier;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Closed tabs `Space t u` can bring back from memory, newest last (20). Closed
/// consoles are also in the trash of the state directory, where `Space t u` finds them once
/// this list is used up, across restarts too.
pub const REOPEN_LIMIT: usize = 20;

/// Stable id of a tab for the life of the process (never reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    /// A scratch SQL console (its text is kept in the state directory).
    Console,
    /// A saved query (a file in the data directory).
    Script,
    /// A table or view opened from the explorer: its query is the document,
    /// the results take the whole tab, and there is no editor to show.
    Table,
    /// An object's DDL, read from the catalog: a read-only editor takes the whole tab, and
    /// there are no results.
    Ddl,
}

/// What a DDL tab shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdlState {
    /// Not read since the tab was restored or brought back: nothing is sent until the user
    /// asks (`r`, the run key).
    NotLoaded,
    /// Waiting for its profile to connect.
    Connecting,
    /// Asked for, not answered yet.
    Loading,
    /// Read: the editor has it.
    Loaded,
    /// Another session's lock was in the way: nothing was read (never a part of the DDL).
    Locked,
    /// Why it could not be read.
    Failed(String),
}

/// A DDL tab's object and where its reading is.
#[derive(Clone, Debug)]
pub struct DdlTab {
    pub object: datarig_core::driver::ddl::DdlObject,
    pub state: DdlState,
    /// The request whose answer the tab waits for: answers to any other one are stale.
    pub request: Option<u64>,
    /// The object as the catalog named it, once read (a name the user typed may differ).
    pub name: Option<String>,
}

impl DdlTab {
    /// `object`, not read yet.
    pub fn new(object: datarig_core::driver::ddl::DdlObject) -> Self {
        Self { object, state: DdlState::NotLoaded, request: None, name: None }
    }

    /// Its name: as the catalog named it once read, else its object's.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.object.label())
    }
}

/// The table or view a table tab shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

impl TableRef {
    /// `schema.name`, as the tab label shows it.
    pub fn label(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    /// The query that reads it in dialect `d`: every column and row, identifiers quoted.
    pub fn query(&self, d: Dialect) -> String {
        format!("SELECT * FROM {}.{}", d.force_quote_ident(&self.schema), d.force_quote_ident(&self.name))
    }
}

/// How a tab lays out its panes: a query tab's results pane below the editor (its share of the
/// height, hidden or shown), and the pane zoomed to the whole workspace, if any. Kept per tab;
/// in `workspace.toml` all but a zoom of another pane than the results (`maximized`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneLayout {
    /// Percent of the editor and results area the results take ([`PaneLayout::MIN`] to
    /// [`PaneLayout::MAX`]).
    pub share: u16,
    /// The user hid the pane; the next run shows it again.
    pub hidden: bool,
    /// The pane zoomed to the whole workspace (tmux style; the others are not drawn), or none.
    /// `Results` is the results pane with its inspector, and also the results' "maximise".
    pub zoom: Option<Focus>,
}

impl PaneLayout {
    pub const DEFAULT_SHARE: u16 = datarig_core::workspace::DEFAULT_SHARE;
    pub const MIN: u16 = 20;
    pub const MAX: u16 = 80;
    /// One key press grows or shrinks the pane by this much.
    pub const STEP: u16 = 5;

    /// `share` within the limits.
    pub fn clamped(share: u16) -> u16 {
        share.clamp(Self::MIN, Self::MAX)
    }
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self { share: Self::DEFAULT_SHARE, hidden: false, zoom: None }
    }
}

/// Where the tab's query session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// No session.
    Idle,
    /// Opened, `Connected` not reported yet (statements sent now wait for it).
    Opening,
    /// Connected.
    Ready,
    /// The connection ended by itself. The next run opens a new session.
    Lost,
}

/// A row result that is not the one shown, with its own place in the grid.
pub struct Parked {
    pub rs: crate::widgets::grid::ResultSet,
    pub grid: GridState,
}

/// What a query tab's results pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultView {
    /// The tab's `results`: a row result (one of the result tabs), or what the last run said.
    #[default]
    Rows,
    /// Every statement of the last run and what it did, and the app's notes about it.
    Messages,
    /// The plan of a statement of the run (`TabSession::plan`).
    Plan,
    /// The shown row result as a chart (`TabSession::chart`).
    Chart,
}

/// The tab's query session and what runs on it.
pub struct TabSession {
    pub session: Option<Session>,
    /// Generation of `session`: events of an older one are dropped.
    pub generation: u64,
    pub state: SessionState,
    pub running: Option<Running>,
    /// The server reports an open transaction on this session.
    pub tx_open: bool,
    /// That transaction is aborted: only `ROLLBACK` works until it ends.
    pub tx_aborted: bool,
    /// The user's own transaction block is open (`DbEvent::Block`): `tx_open` also counts the
    /// transaction that holds a result's portal.
    pub in_block: bool,
    /// Id of the last statement run here (results of other ids are stale).
    pub query_id: u64,
    /// The running statements change the schema (DDL): the profile's key cache is read again
    /// once they end (after the transaction ends, when they ran inside one).
    pub ddl: bool,
    /// DDL ran inside the open transaction: the key cache is read again when it ends.
    pub ddl_in_tx: bool,
    /// A transaction was open when the connection was lost (said once it reconnects).
    pub lost_tx: bool,
    /// Whether the result's portal is open, and since when nobody fetched from it.
    pub paging: Paging,
    /// The statements of the last run and what each one did.
    pub run: super::runlog::RunLog,
    /// The row results of the run they came from that are not shown now, by the statement's
    /// index in that run (one result tab each). The shown one is the tab's
    /// `results`. Each has its own store, which spills like the result's; past its limit the
    /// rest of a statement's rows are read and dropped.
    pub steps: std::collections::BTreeMap<usize, Parked>,
    /// The statement whose rows the tab's `results` holds, when it holds rows.
    pub shown: Option<usize>,
    /// What the results pane shows: a row result (the tab's `results`) or the Messages of the
    /// last run.
    pub view: ResultView,
    /// The run the row results came from, when a later run did not replace them (it returned
    /// no rows): its statements name the result tabs, which say they are from an earlier run.
    pub kept_log: Option<super::runlog::RunLog>,
    /// The running run has not delivered a row result yet: its first one replaces the row
    /// results of the run before.
    pub replace_pending: bool,
    /// The first line of the Messages list on screen.
    pub messages_scroll: usize,
    /// The plan a statement of the row results' run returned (`EXPLAIN (FORMAT JSON)`), and
    /// how it is shown. It goes with those row results.
    pub plan: Option<super::plan::PlanTab>,
    /// The chart of the shown row result and what it draws, once asked for: it follows the
    /// result shown (a later run's too, keeping the columns chosen while they are there).
    pub chart: Option<super::chart::ChartTab>,
    /// Where the run's last statement's result came from: paging past its
    /// closed portal and counting happen only on that profile, binding and session.
    pub origin: Option<super::pages::Origin>,
    /// The statement is being run again to fetch past its closed portal.
    pub resuming: Option<super::pages::Resuming>,
    /// The page to show once the page being fetched arrives.
    pub want_page: Option<usize>,
    /// The driver said the coming first page of the running query is not held
    /// (`DbEvent::Released`): with more rows after it, its paging is `Paging::Released`.
    pub released: bool,
    /// The statement whose rows are being counted.
    pub count_for: Option<usize>,
    /// How many of the user's transactions began in this tab (the open one's number).
    pub tx_epoch: u64,
    /// The result's statement may be run again for the next page past its closed portal (the
    /// plain-`SELECT` allowlist), as found when the portal closed.
    pub rerun_ok: bool,
    /// `session` was opened read-only (its profile's policy was read-only then).
    pub read_only: bool,
    /// The running statements include an `EXPLAIN ANALYZE` of more than a read: the driver
    /// rolls back what it ran, and the tab says so when the run ends well.
    pub explain_rolled_back: bool,
    /// The risk classifier of the tab's language, with the statements `session` prepared
    /// (`PREPARE`), as far as the server confirmed it, and what each one does: an `EXECUTE` is
    /// checked as the statement it runs. A new session starts with none; a new language gets a
    /// new classifier.
    pub prepared: Classifier,
    /// The statements of the running run the server has not answered yet, with their place in
    /// it. Sent is not succeeded: what one prepares, deallocates or discards changes `prepared`
    /// only once the server says it succeeded; when it failed, was cancelled or never ran
    /// (the run stopped before it, the connection was lost), every name it may have touched
    /// is forgotten, so an `EXECUTE` of it asks.
    pub unconfirmed: Vec<(usize, String)>,
    /// Where `session` works, as the server said once it connected: its database
    /// and the schemas of its search path that exist.
    pub context: Option<(String, Vec<String>)>,
    /// `session` sets the tab's search path in each transaction (the server ignored it at
    /// connect, a pooler: `DbEvent::ContextPerTransaction`).
    pub path_per_transaction: bool,
    /// The language `session` said its text is read in (`DbEvent::Language`: a MySQL session's
    /// sql mode); `None` until it says, and for a driver whose sessions do not.
    pub language: Option<datarig_core::sql::dialect::Language>,
}

impl TabSession {
    fn new() -> Self {
        Self {
            session: None,
            generation: 0,
            state: SessionState::Idle,
            running: None,
            tx_open: false,
            tx_aborted: false,
            in_block: false,
            query_id: 0,
            ddl: false,
            ddl_in_tx: false,
            lost_tx: false,
            paging: Paging::None,
            run: super::runlog::RunLog::default(),
            steps: std::collections::BTreeMap::new(),
            shown: None,
            view: ResultView::Rows,
            kept_log: None,
            replace_pending: false,
            messages_scroll: 0,
            plan: None,
            chart: None,
            origin: None,
            resuming: None,
            want_page: None,
            released: false,
            count_for: None,
            tx_epoch: 0,
            rerun_ok: false,
            read_only: false,
            explain_rolled_back: false,
            prepared: Classifier::default(),
            unconfirmed: Vec::new(),
            context: None,
            path_per_transaction: false,
            language: None,
        }
    }

    /// The user's own transaction block is open: what the tab bar's `◆` and the
    /// status bar's "TX open" show. The transaction the driver opens to hold a result's portal
    /// outside the block is not the user's; it shows as connected.
    pub fn user_tx(&self) -> bool {
        self.tx_open && self.in_block
    }

    /// Ending the session now would roll back something the user may want: the
    /// user's block, or the transaction that holds a result's portal when its statement may
    /// have written (anything but a plain `SELECT` the app could run again: an
    /// `INSERT … RETURNING` whose rows page, a function of the user's). Closing the portal of a
    /// plain read loses nothing. What the confirmations and notices of a rollback ask about.
    pub fn tx_at_risk(&self) -> bool {
        self.user_tx()
            || (self.tx_open && self.run.statements.last().is_none_or(|s| self.prepared.repeatable(&s.sql).is_err()))
    }

    /// The server says the running run's statements up to `index` (all of them: `None`)
    /// succeeded: what they prepared and deallocated counts now.
    pub(super) fn succeeded(&mut self, index: Option<usize>) {
        let n = self.unconfirmed.iter().take_while(|(i, _)| index.is_none_or(|last| *i <= last)).count();
        for (_, sql) in self.unconfirmed.drain(..n) {
            self.prepared.classify(&sql);
        }
    }

    /// The running run ended without the server saying its other statements succeeded: the
    /// names they may have touched are unknown now.
    pub(super) fn unsure(&mut self) {
        for (_, sql) in std::mem::take(&mut self.unconfirmed) {
            self.prepared.forget(&sql);
        }
    }
}

pub struct Tab {
    pub id: TabId,
    pub kind: TabKind,
    /// The connection profile the tab runs on.
    pub profile: Option<ProfileId>,
    /// The database and schema of the profile's server the tab works in (the
    /// profile's defaults unless chosen). Part of the binding: changing it is a rebind.
    pub context: SessionContext,
    /// Tab generation: a new value each time the tab is (re)bound to a profile. A statement
    /// queued for the tab's connection runs only if the tab still has the binding it was
    /// queued under.
    pub binding: u64,
    pub editor: Editor,
    /// Auto-completion popup of the editor.
    pub popup: Option<Popup>,
    /// When the debounced auto-completion pops up.
    pub completion_due: Option<Instant>,
    pub results: Results,
    pub grid: GridState,
    pub exec: TabSession,
    /// The last statement outcome of this tab (the status bar shows it while the tab is active).
    pub status: Option<Notice>,
    /// Where the tab's text is kept.
    pub doc: Doc,
    /// The results pane's place below the editor (a query tab).
    pub pane: PaneLayout,
    /// A statement was run in the tab (since it opened or was restored): a query tab shows its
    /// results pane from then on, unless the user hides it.
    pub ran: bool,
}

/// Where a tab's text is kept, and what is on disk.
pub struct Doc {
    /// The console's file name in the state directory (`consoles/<id>.sql`).
    pub console_id: String,
    /// A saved query: its path in the scripts directory.
    pub script: Option<String>,
    /// The text as last written or read.
    pub saved: String,
    /// The editor's text version when it was last written (the text is `saved` while the
    /// editor still has this version).
    pub saved_version: Option<u64>,
    /// When the oldest edit not written yet was made.
    pub unsaved_since: Option<Instant>,
    /// The text was written (or read) at least once: a new console has no file yet.
    pub written: bool,
    /// The script file's stamp as last written or read (a save checks it first).
    pub stamp: Option<Stamp>,
    /// When the next autosave is due (at most [`crate::app::AUTOSAVE`] after an edit).
    pub save_due: Option<Instant>,
    /// Why the last save failed (the tab shows it is not saved; the next one tries again).
    pub save_error: Option<datarig_core::i18n::Msg>,
    /// The script changed or went away on disk: nothing is written until the user decides.
    pub conflict: bool,
    /// Restored or opened without connecting: the profile connects once the tab has the focus.
    pub lazy_connect: bool,
    /// Brought back from a console file the saved workspace did not list (the tab bar says
    /// so).
    pub recovered: bool,
    /// The profile the tab was saved with when that profile is not among the profiles (the
    /// config file cannot be read, or no longer lists it): the tab has no connection, but the
    /// binding is written back as it is, so it works again once the profile is back. Only
    /// binding the tab to a profile or deleting that profile in the app drops it.
    pub kept_profile: Option<ProfileId>,
    /// A console's number in its name (`console 3`): the lowest one no other open tab had when
    /// it opened, kept in `workspace.toml` (0: not numbered yet).
    pub console_no: u32,
    /// The table a table tab shows.
    pub table: Option<TableRef>,
    /// The object a DDL tab shows.
    pub ddl: Option<DdlTab>,
}

impl Doc {
    fn new() -> Self {
        Self {
            console_id: datarig_core::workspace::new_id(),
            script: None,
            saved: String::new(),
            saved_version: None,
            unsaved_since: None,
            written: false,
            stamp: None,
            save_due: None,
            save_error: None,
            conflict: false,
            lazy_connect: false,
            recovered: false,
            kept_profile: None,
            console_no: 0,
            table: None,
            ddl: None,
        }
    }
}

impl Tab {
    /// Its session sets the tab's search path in each transaction (a pooler): a
    /// statement of the run that sets `search_path` for the session (`risk::Risk::session_path`,
    /// read from the parse tree) is not blocked, but the run's Messages say once that the tab
    /// ignores it and that it stays on the pooled connection for other clients (the status bar
    /// too when the run ends). An `EXECUTE` is read through the session's prepared
    /// statements, as the run's own `PREPARE`s change them.
    pub fn warn_session_path(&mut self) {
        use super::Level;
        use datarig_core::i18n::Msg;
        let schema = self.context.schema.clone().unwrap_or_default();
        let warning = Msg::ContextSessionPath { schema };
        if self.exec.run.notes.iter().any(|n| n.msg == warning) {
            return;
        }
        // Only a text that names the setting or `set_config` can set it, and only one that
        // prepares, runs or drops a prepared statement changes what an `EXECUTE` does (a cheap
        // look first).
        const WORDS: [&str; 6] = ["search_path", "set_config", "execute", "prepare", "deallocate", "discard"];
        let may = |sql: &str| {
            let l = sql.to_ascii_lowercase();
            WORDS.iter().any(|w| l.contains(w))
        };
        let mut classifier = self.exec.prepared.clone();
        let sets = self.exec.run.statements.iter().any(|s| may(&s.sql) && classifier.classify(&s.sql).session_path);
        if sets {
            self.exec.run.notes.push(Notice::new(warning, Level::Warning));
        }
    }

    /// The run's warning about a session-level `search_path` ([`Tab::warn_session_path`]), if
    /// it has one: the status bar's message when the run ends.
    pub fn session_path_note(&self) -> Option<Notice> {
        use datarig_core::i18n::Msg;
        self.exec.run.notes.iter().find(|n| matches!(n.msg, Msg::ContextSessionPath { .. })).cloned()
    }

    pub fn new(id: TabId, kind: TabKind, profile: Option<ProfileId>, editor: Editor) -> Self {
        Self {
            id,
            kind,
            profile,
            context: SessionContext::default(),
            binding: 0,
            editor,
            popup: None,
            completion_due: None,
            results: Results::Empty,
            grid: GridState::default(),
            exec: TabSession::new(),
            status: None,
            doc: Doc::new(),
            pane: PaneLayout::default(),
            ran: false,
        }
    }

    /// Put `editor` in place of the tab's editor, in the tab's language.
    pub fn replace_editor(&mut self, mut editor: Editor) {
        editor.set_language(self.editor.language());
        self.editor = editor;
    }

    /// A table tab (no editor on screen, the results take the whole tab).
    pub fn is_table(&self) -> bool {
        self.kind == TabKind::Table
    }

    /// A DDL tab (a read-only editor takes the whole tab, no results).
    pub fn is_ddl(&self) -> bool {
        self.kind == TabKind::Ddl
    }

    /// A query tab: a console or a saved query, whose text is the user's (it is saved, it
    /// runs, its connection can change).
    pub fn is_query(&self) -> bool {
        matches!(self.kind, TabKind::Console | TabKind::Script)
    }

    /// The run the row results came from: an earlier run when the last one returned none.
    pub fn rows_log(&self) -> &super::runlog::RunLog {
        self.exec.kept_log.as_ref().unwrap_or(&self.exec.run)
    }

    /// The statement whose rows the tab's `results` holds (empty when it holds none).
    pub fn shown_sql(&self) -> &str {
        self.exec.shown.and_then(|i| self.rows_log().statements.get(i)).map_or("", |s| s.sql.as_str())
    }

    /// The statements that have a row result, in their order (the result tabs).
    pub fn result_tabs(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.exec.steps.keys().copied().collect();
        if let (Some(i), super::Results::Rows(_)) = (self.exec.shown, &self.results) {
            v.push(i);
        }
        v.sort_unstable();
        v
    }

    /// Show the row result of statement `i` (the shown one is parked with its grid first).
    pub fn show_result(&mut self, i: usize) {
        self.exec.view = ResultView::Rows;
        if self.exec.shown == Some(i) && matches!(self.results, super::Results::Rows(_)) {
            return;
        }
        let Some(next) = self.exec.steps.remove(&i) else { return };
        self.park_shown();
        self.results = super::Results::Rows(next.rs);
        self.grid = next.grid;
        self.exec.shown = Some(i);
    }

    /// Put the shown row result with the others (the tab's `results` then holds none).
    pub fn park_shown(&mut self) {
        if let (Some(j), super::Results::Rows(_)) = (self.exec.shown, &self.results) {
            let super::Results::Rows(rs) = std::mem::replace(&mut self.results, super::Results::Empty) else {
                return;
            };
            let grid = std::mem::take(&mut self.grid);
            self.exec.steps.insert(j, Parked { rs, grid });
        }
        self.exec.shown = None;
    }

    /// The row results of the run before go (the running run delivered its first rows).
    pub fn drop_rows(&mut self) {
        self.exec.steps.clear();
        self.exec.plan = None;
        self.exec.kept_log = None;
        self.exec.shown = None;
        self.exec.replace_pending = false;
        self.exec.origin = None;
        self.exec.paging = super::Paging::None;
        if matches!(self.results, super::Results::Rows(_)) {
            self.results = super::Results::Empty;
        }
    }

    /// The run ended (`failed`: it failed or was cancelled): a run that delivered no rows leaves
    /// the earlier ones (they say so); the pane shows the run's last result tab when it has
    /// rows and did not fail, else its Messages. A table tab shows its result or its error.
    pub fn run_ended(&mut self, failed: bool) {
        self.exec.replace_pending = false;
        if self.is_table() {
            self.exec.view = ResultView::Rows;
            return;
        }
        let answered = self.exec.kept_log.is_none()
            && self.exec.shown.is_some_and(|i| i + 1 == self.exec.run.len())
            && matches!(self.results, super::Results::Rows(_));
        let last = self.result_tabs().last().copied().filter(|_| self.exec.kept_log.is_none());
        match last {
            _ if failed => self.exec.view = ResultView::Messages,
            // A chart shown before the run shows the run's rows.
            _ if answered && self.exec.view == ResultView::Chart && self.exec.chart.is_some() => {}
            _ if answered => self.exec.view = ResultView::Rows,
            Some(i) => self.show_result(i),
            None => self.exec.view = ResultView::Messages,
        }
        // The rows shown are a plan: the plan instead.
        if self.exec.view == ResultView::Rows
            && self.exec.plan.as_ref().is_some_and(|p| Some(p.index) == self.exec.shown)
        {
            self.exec.view = ResultView::Plan;
        }
    }

    /// Every row result the tab keeps (shown or parked).
    pub fn row_results_mut(&mut self) -> impl Iterator<Item = &mut crate::widgets::grid::ResultSet> {
        let shown = match &mut self.results {
            super::Results::Rows(rs) => Some(rs),
            _ => None,
        };
        shown.into_iter().chain(self.exec.steps.values_mut().map(|p| &mut p.rs))
    }

    /// The tab's session was closed under it (a disconnect, the profile connected again, its
    /// attempt failed): nothing that session would answer is read. A run in progress ends here,
    /// cancelled, once, with the reason in its Messages; a count or a fetch of more rows stops
    /// and says so (Messages and the tab's status); no portal stays open, and the user's transaction is gone (the next run says so).
    /// Whether a run ended.
    pub fn session_closed(&mut self) -> bool {
        self.exec.lost_tx |= self.exec.tx_at_risk();
        let ended = match self.exec.running.take() {
            Some(r) if !r.fetch && !r.count => {
                self.exec.run.answered(super::runlog::StatementOutcome::Cancelled, None);
                let note = Notice::new(datarig_core::i18n::Label::QueryCancelledSessionClosed, super::Level::Warning);
                self.exec.run.notes.push(note);
                if !matches!(self.results, Results::Rows(_)) {
                    self.results = Results::Cancelled;
                }
                self.run_ended(true);
                true
            }
            // A count or a fetch of more rows stops; said where it was announced, so its
            // announcement is not the last word.
            Some(r) => {
                let label = if r.count {
                    datarig_core::i18n::Label::ResultsCountSessionClosed
                } else {
                    datarig_core::i18n::Label::ResultsFetchSessionClosed
                };
                let note = Notice::new(label, super::Level::Warning);
                self.exec.run.notes.push(note.clone());
                self.status = Some(note);
                false
            }
            None => false,
        };
        self.exec.resuming = None;
        self.exec.want_page = None;
        self.exec.paging = Paging::None;
        self.exec.tx_open = false;
        // The server rolls the user's transaction back when its session ends.
        self.block_ended(true);
        ended
    }

    /// The user's transaction began.
    pub fn block_began(&mut self) {
        if !self.exec.in_block {
            self.exec.tx_epoch += 1;
            self.exec.in_block = true;
        }
    }

    /// The user's transaction ended; `rolled_back` when the app knows it rolled back. Results
    /// read in it say it ended.
    pub fn block_ended(&mut self, rolled_back: bool) {
        if !self.exec.in_block {
            return;
        }
        self.exec.in_block = false;
        let epoch = self.exec.tx_epoch;
        for rs in self.row_results_mut() {
            rs.tx_ended(epoch, rolled_back);
        }
    }

    /// Whether the statement that ended the user's transaction rolled it back, as far as the
    /// app can tell: `ROLLBACK`/`ABORT`, a `COMMIT`/`END` of an aborted block, a `COMMIT` that
    /// failed (the server rolls back). Anything else (a `COMMIT`, a `PREPARE TRANSACTION`, what
    /// the app cannot tell) is not known to have rolled back.
    pub fn ending_rolled_back(&self) -> bool {
        use super::runlog::StatementOutcome as O;
        let log = &self.exec.run;
        let Some(s) = self.ending_statement().and_then(|i| log.statements.get(i)) else { return false };
        let words: Vec<String> = datarig_core::sql::lexer::lex_in(&s.sql, self.exec.prepared.language().dialect())
            .into_iter()
            .filter(|t| t.is_word())
            .take(2)
            .map(|t| t.text(&s.sql).to_ascii_uppercase())
            .collect();
        let first = words.first().map(String::as_str);
        let to = words.get(1).is_some_and(|w| w == "TO");
        match first {
            Some("ROLLBACK" | "ABORT") => !to,
            Some("COMMIT" | "END") => self.exec.tx_aborted || matches!(s.outcome, O::Failed(_)),
            _ => false,
        }
    }

    /// The statement of the last run that ended the user's transaction: the running one, else
    /// the last that ran.
    pub fn ending_statement(&self) -> Option<usize> {
        use super::runlog::StatementOutcome as O;
        let log = &self.exec.run;
        log.running().or_else(|| log.statements.iter().rposition(|s| !matches!(s.outcome, O::Waiting | O::NotRun)))
    }

    /// The shown row result.
    pub fn shown_rows_mut(&mut self) -> Option<&mut crate::widgets::grid::ResultSet> {
        match &mut self.results {
            super::Results::Rows(rs) => Some(rs),
            _ => None,
        }
    }

    /// The row result of statement `i` and its grid, shown or parked.
    pub fn rows_of_mut(&mut self, i: usize) -> Option<(&mut crate::widgets::grid::ResultSet, &mut GridState)> {
        match (&mut self.results, self.exec.shown) {
            (super::Results::Rows(rs), Some(s)) if s == i => Some((rs, &mut self.grid)),
            _ => self.exec.steps.get_mut(&i).map(|p| (&mut p.rs, &mut p.grid)),
        }
    }

    /// The statement whose rows page (the last one of their run).
    pub fn answer_index(&self) -> Option<usize> {
        self.rows_log().len().checked_sub(1)
    }

    /// The text of that statement.
    pub fn answer_sql(&self) -> &str {
        self.rows_log().statements.last().map_or("", |s| s.sql.as_str())
    }

    /// The rows of the run's last statement (the one that pages), wherever they are.
    pub fn answer_rows_mut(&mut self) -> Option<(&mut crate::widgets::grid::ResultSet, &mut GridState)> {
        let answer = self.answer_index()?;
        self.rows_of_mut(answer)
    }

    /// The text differs from what is on disk (or was never written).
    pub fn dirty(&self) -> bool {
        if self.doc.written && self.doc.saved_version == Some(self.editor.version()) {
            return false;
        }
        !self.doc.written || self.editor.text() != self.doc.saved
    }

    /// The saved query's path, if the tab is one.
    pub fn script(&self) -> Option<&str> {
        self.doc.script.as_deref()
    }

    /// The profile binding as `workspace.toml` and `scripts.toml` keep it: the tab's profile,
    /// else the one it keeps while that profile is unknown ([`Doc::kept_profile`]).
    pub fn saved_profile(&self) -> Option<ProfileId> {
        self.profile.or(self.doc.kept_profile)
    }
}

/// What `Space t u` needs to bring a closed tab back.
pub struct ClosedTab {
    pub kind: TabKind,
    /// A saved query's path (it is read again from disk when it comes back).
    pub script: Option<String>,
    pub console_id: String,
    pub profile: Option<ProfileId>,
    /// See [`Doc::kept_profile`].
    pub kept_profile: Option<ProfileId>,
    pub text: String,
    pub cursor: (usize, usize),
    /// Where it was (clamped when it comes back).
    pub index: usize,
    /// A console's file in the trash (see `workspace::trash_console`), once it is there.
    pub trashed: Option<String>,
    /// A table tab's table.
    pub table: Option<TableRef>,
    /// A DDL tab's object.
    pub ddl: Option<datarig_core::driver::ddl::DdlObject>,
    pub pane: PaneLayout,
    pub context: SessionContext,
    /// A console's number as its tab showed it (0: not a console): held while it is on the
    /// closed list, so it comes back with it (should another tab have it, a free one).
    pub console_no: u32,
    /// Which closed tab this is, for the life of the process (the tab list picks one by it).
    pub serial: u64,
}

/// The open tabs in display order. There may be none (the workspace then shows its empty
/// state); [`TabManager::active`] is then a blank stand-in that is never drawn and belongs to
/// no profile, and the actions that need a tab are not available (`app::action`).
pub struct TabManager {
    tabs: Vec<Tab>,
    active: usize,
    /// What [`TabManager::active`] gives while there is no tab.
    blank: Tab,
    next_id: u64,
    /// Last query-session generation handed out (shared by all tabs, never reused).
    generation: u64,
    /// Closed tabs, newest last.
    closed: VecDeque<ClosedTab>,
    /// The tab each profile used last.
    last_used: HashMap<ProfileId, TabId>,
    /// The tabs made active, most recent first (this run only; see [`TabManager::by_recent`]).
    recent: Vec<TabId>,
}

impl Default for TabManager {
    /// No tab.
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            // Id 0 is never handed out, so nothing ever finds the blank tab by its id.
            blank: Tab::new(TabId(0), TabKind::Console, None, Editor::new("")),
            next_id: 1,
            generation: 0,
            closed: VecDeque::new(),
            last_used: HashMap::new(),
            recent: Vec::new(),
        }
    }
}

impl TabManager {
    /// One console tab with `editor`.
    pub fn new(profile: Option<ProfileId>, editor: Editor) -> Self {
        let mut m = Self::default();
        m.open(TabKind::Console, profile, editor);
        m
    }

    fn new_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    /// A new query-session generation: events of older sessions are dropped.
    pub fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Open a tab right after the active one and make it active.
    pub fn open(&mut self, kind: TabKind, profile: Option<ProfileId>, editor: Editor) -> TabId {
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.insert(at, kind, profile, editor)
    }

    fn insert(&mut self, at: usize, kind: TabKind, profile: Option<ProfileId>, editor: Editor) -> TabId {
        let id = self.new_id();
        let at = at.min(self.tabs.len());
        let mut tab = Tab::new(id, kind, profile, editor);
        tab.binding = self.next_generation();
        if kind == TabKind::Console {
            tab.doc.console_no = self.free_console_no();
        }
        self.tabs.insert(at, tab);
        self.activate(at);
        id
    }

    /// Make tab `index` active; `false` when there is no such tab.
    pub fn activate(&mut self, index: usize) -> bool {
        let Some(t) = self.tabs.get(index) else { return false };
        if let Some(p) = t.profile {
            self.last_used.insert(p, t.id);
        }
        let id = t.id;
        self.recent.retain(|r| *r != id);
        self.recent.insert(0, id);
        self.active = index;
        true
    }

    /// Move `delta` tabs to the right (negative: left), wrapping around.
    pub fn cycle(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n == 0 {
            return;
        }
        self.activate((self.active as isize + delta).rem_euclid(n) as usize);
    }

    /// Remove tab `id` and remember it for [`TabManager::reopen`]. The active tab becomes the
    /// one to its right, or the new last one; closing the last tab leaves none.
    pub fn close(&mut self, id: TabId) -> Option<Tab> {
        let index = self.position(id)?;
        let tab = self.tabs.remove(index);
        self.last_used.retain(|_, t| *t != id);
        self.recent.retain(|r| *r != id);
        let serial = self.new_id().0;
        self.closed.push_back(ClosedTab {
            kind: tab.kind,
            script: tab.doc.script.clone(),
            console_id: tab.doc.console_id.clone(),
            profile: tab.profile,
            kept_profile: tab.doc.kept_profile,
            text: tab.editor.text(),
            cursor: (tab.editor.row, tab.editor.col),
            index,
            trashed: None,
            table: tab.doc.table.clone(),
            ddl: tab.doc.ddl.as_ref().map(|d| d.object.clone()),
            pane: tab.pane,
            context: tab.context.clone(),
            console_no: tab.doc.console_no,
            serial,
        });
        while self.closed.len() > REOPEN_LIMIT {
            self.closed.pop_front();
        }
        let active = if self.active > index || self.active == self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        if !self.tabs.is_empty() {
            self.activate(active.min(self.tabs.len() - 1));
        } else {
            self.active = 0;
        }
        Some(tab)
    }

    /// The most recently closed tab, taken off the list (see [`TabManager::reopen`]).
    pub fn take_closed(&mut self) -> Option<ClosedTab> {
        self.closed.pop_back()
    }

    /// Closed tab `serial`, taken off the list.
    pub fn take_closed_serial(&mut self, serial: u64) -> Option<ClosedTab> {
        let i = self.closed.iter().position(|c| c.serial == serial)?;
        self.closed.remove(i)
    }

    /// The closed tabs, newest first.
    pub fn closed(&self) -> impl Iterator<Item = &ClosedTab> {
        self.closed.iter().rev()
    }

    /// The open tabs by when they were last active: the active one, the ones made active
    /// before it from the most recent, then the ones not made active in this run in the tab
    /// bar's order.
    pub fn by_recent(&self) -> Vec<TabId> {
        let mut order: Vec<TabId> = Vec::with_capacity(self.tabs.len());
        if !self.tabs.is_empty() {
            order.push(self.active().id);
        }
        for id in self.recent.iter().chain(self.tabs.iter().map(|t| &t.id)) {
            if !order.contains(id) && self.get(*id).is_some() {
                order.push(*id);
            }
        }
        order
    }

    /// The most recently closed tab (still on the list).
    pub fn last_closed_mut(&mut self) -> Option<&mut ClosedTab> {
        self.closed.back_mut()
    }

    /// Drop the closed tab whose console is trashed file `name` (it was brought back from the
    /// trash another way).
    pub fn forget_trashed(&mut self, name: &str) {
        self.closed.retain(|c| c.trashed.as_deref() != Some(name));
    }

    /// Bring the most recently closed tab back where it was (a new id and no session).
    pub fn reopen(&mut self) -> Option<TabId> {
        let c = self.closed.pop_back()?;
        Some(self.reopen_closed(c))
    }

    /// Bring closed tab `c` back where it was (a new id and no session).
    pub fn reopen_closed(&mut self, c: ClosedTab) -> TabId {
        let mut editor = Editor::new(&c.text);
        editor.row = c.cursor.0;
        editor.col = c.cursor.1;
        let id = self.insert(c.index, c.kind, c.profile, editor);
        // A console keeps its number unless another tab took it meanwhile.
        let taken = self.tabs.iter().any(|t| t.id != id && t.doc.console_no == c.console_no);
        if let Some(t) = self.get_mut(id) {
            if t.kind == TabKind::Console && c.console_no != 0 && !taken {
                t.doc.console_no = c.console_no;
            }
            t.doc.console_id = c.console_id;
            t.doc.script = c.script;
            t.doc.kept_profile = c.kept_profile;
            t.doc.table = c.table;
            t.pane = c.pane;
            t.context = c.context;
            // Not read again by itself: `r` reads it.
            if let Some(object) = c.ddl {
                t.replace_editor(Editor::read_only(""));
                t.doc.ddl = Some(DdlTab::new(object));
            }
            if t.is_table() || t.is_ddl() {
                // Not a text anybody wrote (a table's query, an object's DDL): nothing to save.
                t.doc.saved = t.editor.text();
                t.doc.written = true;
            }
        }
        id
    }

    /// The lowest console number no open tab and no console on the closed list has.
    pub fn free_console_no(&self) -> u32 {
        let held = |n: u32| {
            self.tabs.iter().any(|t| t.doc.console_no == n)
                || self.closed.iter().any(|c| c.kind == TabKind::Console && c.console_no == n)
        };
        (1..).find(|n| !held(*n)).unwrap_or(1)
    }

    /// Tab `id` became a console (a saved query whose file went away): it gets a number.
    pub fn number_console(&mut self, id: TabId) {
        let no = self.free_console_no();
        if let Some(t) = self.get_mut(id).filter(|t| t.doc.console_no == 0) {
            t.doc.console_no = no;
        }
    }

    /// The table tab of `table` on `profile` in `database` (`None`: the profile's own), if one
    /// is open.
    pub fn find_table(&self, profile: ProfileId, database: Option<&str>, table: &TableRef) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|t| {
                t.is_table()
                    && t.profile == Some(profile)
                    && t.context.database.as_deref() == database
                    && t.doc.table.as_ref() == Some(table)
            })
            .map(|t| t.id)
    }

    /// The DDL tab of `object` on `profile` in `database` (`None`: the profile's own), if one
    /// is open: asked for as `object`, or read as what `object` names (a name typed with `:ddl`
    /// that turned out to be that relation).
    pub fn find_ddl(
        &self,
        profile: ProfileId,
        database: Option<&str>,
        object: &datarig_core::driver::ddl::DdlObject,
    ) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|t| {
                t.profile == Some(profile)
                    && t.context.database.as_deref() == database
                    && t.doc.ddl.as_ref().is_some_and(|d| d.object == *object || d.name == Some(object.label()))
            })
            .map(|t| t.id)
    }

    /// Profile `profile` was deleted: closed tabs bound to it come back without a connection.
    pub fn unbind_closed(&mut self, profile: ProfileId) {
        for c in &mut self.closed {
            if c.profile == Some(profile) {
                c.profile = None;
            }
            if c.kept_profile == Some(profile) {
                c.kept_profile = None;
            }
        }
    }

    /// Put tab `tab` at `index` (restoring a workspace; it keeps its id and is not activated).
    pub fn insert_tab(&mut self, index: usize, mut tab: Tab) -> TabId {
        let id = self.new_id();
        tab.id = id;
        tab.binding = self.next_generation();
        // A console keeps its number unless another tab has it (or it has none yet).
        let taken = self.tabs.iter().any(|t| t.doc.console_no == tab.doc.console_no);
        if tab.kind == TabKind::Console && (tab.doc.console_no == 0 || taken) {
            tab.doc.console_no = self.free_console_no();
        }
        self.tabs.insert(index.min(self.tabs.len()), tab);
        id
    }

    /// The tab of saved query `script` (its path compared ignoring case).
    pub fn find_script(&self, script: &str) -> Option<TabId> {
        let key = datarig_core::scripts::name::fold(script);
        self.tabs
            .iter()
            .find(|t| t.doc.script.as_deref().is_some_and(|s| datarig_core::scripts::name::fold(s) == key))
            .map(|t| t.id)
    }

    /// Tab `id` runs on `profile` from now on, with the profile's defaults (a new tab
    /// generation).
    pub fn bind(&mut self, id: TabId, profile: Option<ProfileId>) {
        self.bind_in(id, profile, SessionContext::default());
    }

    /// Tab `id` runs on `profile` in `context` from now on (a new tab generation: what was
    /// bound to the old one is refused).
    pub fn bind_in(&mut self, id: TabId, profile: Option<ProfileId>, context: SessionContext) {
        let active = self.active().id == id;
        let binding = self.next_generation();
        let Some(t) = self.get_mut(id) else { return };
        t.profile = profile;
        t.context = context;
        t.doc.kept_profile = None;
        t.binding = binding;
        self.last_used.retain(|_, t| *t != id);
        if let (Some(p), true) = (profile, active) {
            self.last_used.insert(p, id);
        }
    }

    /// Closed tabs that [`TabManager::reopen`] can bring back.
    pub fn closed_count(&self) -> usize {
        self.closed.len()
    }

    /// The tab `profile` used last (the active one if it belongs to the profile).
    pub fn recent_for(&self, profile: ProfileId) -> Option<TabId> {
        let active = self.active();
        if active.profile == Some(profile) {
            return Some(active.id);
        }
        self.last_used
            .get(&profile)
            .copied()
            .filter(|id| self.get(*id).is_some())
            .or_else(|| self.tabs.iter().find(|t| t.profile == Some(profile)).map(|t| t.id))
    }

    pub fn position(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// The active tab (the blank stand-in while there is none).
    pub fn active(&self) -> &Tab {
        self.tabs.get(self.active).unwrap_or(&self.blank)
    }

    /// The active tab (while there is none, a fresh blank stand-in: what is written to it is
    /// dropped at the next call, never shown or saved).
    pub fn active_mut(&mut self) -> &mut Tab {
        if self.tabs.is_empty() {
            self.blank = Tab::new(TabId(0), TabKind::Console, None, Editor::new(""));
            return &mut self.blank;
        }
        &mut self.tabs[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

#[cfg(test)]
mod tests;
