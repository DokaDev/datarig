//! Explicit pagination: the grid shows one page of a result at a time.
//! Pages already fetched come from the result's rows (in memory or its spill file); the page
//! past them is fetched while the portal is open, or, once it closed or when it was never held
//! (`paging = "no_hold"`), by running the statement again when it is on the plain-`SELECT`
//! allowlist (`sql::risk::repeat`), which the app announces; anything else is refused with the
//! reason. The rows are counted only when the user
//! asks, with a `SELECT count(*)` of the same allowlist. Both go through the same read-only and
//! confirm checks as a run, on the session the result came from.

use super::tabs::ResultView;
use super::*;
use datarig_core::sql::risk::repeat::NotRepeatable;

/// Where a result came from: its tab's profile, the tab's binding and its session's generation
/// when its first page arrived. Paging past a closed portal and counting happen only there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub profile: ProfileId,
    pub binding: u64,
    pub generation: u64,
}

/// A statement run again to fetch the page after `skip` rows (a closed portal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resuming {
    /// The columns the result has (name and type): the rows run again must have the same.
    pub columns: Vec<(String, String)>,
    pub skip: u64,
    /// The page to show once the rows are there.
    pub page: usize,
}

/// The reason a statement is not run again or counted for the user, in words.
pub(super) fn why(r: &NotRepeatable) -> Msg {
    match r {
        NotRepeatable::NotOne => Label::RepeatNotOne.into(),
        NotRepeatable::NotSelect => Label::RepeatNotSelect.into(),
        NotRepeatable::Writes => Label::RepeatWrites.into(),
        NotRepeatable::UserFunction => Label::RepeatUserFunction.into(),
        NotRepeatable::Volatile(name) => Msg::RepeatVolatile { name: name.clone() },
        NotRepeatable::UserOperator(name) => Msg::RepeatUserOperator { name: name.clone() },
        NotRepeatable::UserType(name) => Msg::RepeatUserType { name: name.clone() },
        NotRepeatable::NotATable(name) => Msg::RepeatNotATable { name: name.clone() },
        NotRepeatable::RowSecurity(name) => Msg::RepeatRowSecurity { name: name.clone() },
        NotRepeatable::Shadowed(name) => Msg::RepeatShadowed { name: name.clone() },
        NotRepeatable::UserColumnType(name) => Msg::RepeatUserColumnType { name: name.clone() },
        NotRepeatable::PlannerCode(name) => Msg::RepeatPlannerCode { name: name.clone() },
        NotRepeatable::TxUnknown => Label::RepeatTxUnknown.into(),
        NotRepeatable::Unreadable => Label::RepeatUnreadable.into(),
        NotRepeatable::Variable(name) => Msg::RepeatVariable { name: name.clone() },
    }
}

/// Tab `t`'s shown result, run again with `sql`, may not match the rows it has: `sql` reads the
/// session's time zone (a filter on a MySQL `TIMESTAMP`, [`Classifier::reads_zone`]) and the
/// result's rows are not known to be read in the zone the session has now (it changed since,
/// or between its pages, or what it was is not known).
pub(super) fn zone_moved(t: &Tab, sql: &str) -> bool {
    if !t.exec.prepared.reads_zone(sql) {
        return false;
    }
    let read_in = match &t.results {
        Results::Rows(rs) => Some(&rs.read_in),
        _ => None,
    };
    !matches!(
        (read_in, &t.exec.values),
        (Some(datarig_core::driver::ReadIn::Values(then)), Some(now)) if then.zone_known() && then.time_zone == now.time_zone
    )
}

/// A statement (in `lang`) that changes rows and keeps the change (not an `EXPLAIN ANALYZE`,
/// whose changes are rolled back).
fn commits_a_write(lang: Language, sql: &str) -> bool {
    let risk = Classifier::classify_once(lang, sql);
    risk.writes && !risk.rolls_back()
}

/// Why a result not held shows `count` rows only (its statement `sql`, in `lang`, is not run
/// again for the user, for `r`): a statement that changes rows ran to its end and is committed
/// (running it again would change rows again), anything else stopped after its first page.
pub(super) fn first_page_only(i18n: &I18n, lang: Language, sql: &str, r: &NotRepeatable, count: u64) -> Msg {
    if commits_a_write(lang, sql) {
        Msg::ResultsFirstPageOnlyWrite { count }
    } else {
        Msg::ResultsFirstPageOnly { count, why: i18n.msg(&why(r)).to_string() }
    }
}

impl App {
    /// The origin of tab `t`'s results as they would be recorded now.
    pub(super) fn origin_now(t: &Tab) -> Option<Origin> {
        Some(Origin { profile: t.profile?, binding: t.binding, generation: t.exec.generation })
    }

    /// The shown result is the run's last statement (the one with a portal), on the session and
    /// binding it came from.
    pub(super) fn answer_here(&self) -> bool {
        let t = self.tab();
        t.exec.kept_log.is_none()
            && t.exec.shown.is_some_and(|i| i + 1 == t.exec.run.len())
            && t.exec.session.is_some()
            && t.exec.origin.is_some()
            && t.exec.origin == Self::origin_now(t)
    }

    /// The active tab's shown result may be run again for its next page (`n`): it is on the
    /// allowlist and still on the session and binding it came from. After a switch of the tab's
    /// connection or context `n` is refused, so its hint is not shown.
    pub fn can_run_again(&self) -> bool {
        self.tab().exec.rerun_ok && (self.answer_here() || self.kept_answer_here())
    }

    /// The active tab's shown result may be counted (`#`), as far as is known without reading
    /// its statement again: nothing runs in the tab, the result is on the session and binding
    /// it came from, and, when its portal is closed, its statement is on the allowlist (it was
    /// checked when the portal closed). While the portal is open the statement has not been
    /// checked: `#` may then be refused, with the reason.
    pub fn may_count(&self) -> bool {
        let t = self.tab();
        let open = matches!(t.exec.paging, Paging::Open { .. });
        t.exec.running.is_none() && (open || t.exec.rerun_ok) && (self.answer_here() || self.kept_answer_here())
    }

    /// The server's estimate of the rows of the table that table tab `t` shows, as the explorer
    /// read it with the table's schema or structure (nothing is asked for it), at most the
    /// tab's cap on its rows. Only while the rows shown are of every row of the tab's table (in
    /// any order: no filter), and only from the explorer of the profile and database the tab
    /// works in. `None` when the server has none (never analyzed), and for filtered rows (how
    /// many match is not known).
    pub(crate) fn table_estimate(&self, t: &Tab) -> Option<u64> {
        let table = t.doc.table.as_ref().filter(|_| t.is_table())?;
        let shown = t.doc.table_query.as_ref()?.shown.as_ref()?;
        let every = datarig_core::query::Query {
            limit: shown.limit,
            ..datarig_core::query::Query::table(self.table_name(t.id)?)
        };
        if !super::table_query::same_rows(shown, &every) {
            return None;
        }
        let id = t.profile?;
        let tree = match t.context.database.as_deref() {
            None => &self.conns.get(id)?.tree,
            Some(db) => &self.conns.aux(id, db)?.tree,
        };
        let rows = tree.stats_of(&table.schema, &table.name)?.rows?;
        Some(shown.limit.map_or(rows, |n| rows.min(n)))
    }

    /// `n`: the next page of the shown result. A page already fetched is shown at once; the one
    /// past them is fetched (the portal is open) or read by running the statement again (it
    /// closed), and shown when it arrives.
    pub(super) fn page_next(&mut self) {
        let size = self.page_size.max(1);
        let t = self.tab();
        if t.exec.view != ResultView::Rows {
            return;
        }
        let Results::Rows(rs) = &t.results else { return };
        let (len, more) = (rs.rows.len(), rs.more);
        let next = t.grid.window(len).start / size + 1;
        if next * size < len {
            self.tab_mut().grid.set_page(next);
            return;
        }
        if t.exec.running.is_some() {
            return self.flash_busy();
        }
        if !more {
            // A table tab's rows that end at its cap: the limit ended them, not the table.
            let cap = self.table_query(t.id).and_then(|q| q.shown.as_ref()).and_then(|q| q.limit);
            if let Some(count) = cap.filter(|n| *n == len as u64) {
                return self.flash(Notice::new(Msg::ResultsPageLimitReached { count }, Level::Info));
            }
            return self.flash(Notice::new(Label::ResultsPageLast, Level::Info));
        }
        // An Apply changed the table under these rows (a deleted row moves the rows after it
        // up, an added one may move them down): the statement run again past the rows read
        // would skip or repeat rows. Only a run of the query shows the rest.
        let applied = rs.edit.applied.is_some();
        let (id, paging) = (t.id, t.exec.paging);
        match paging {
            Paging::Open { .. } if self.answer_here() => {
                self.tab_mut().exec.want_page = Some(next);
                self.fetch_page(id);
            }
            p if p.closed() && applied => self.flash(Notice::new(Label::ResultsPageApplied, Level::Warning)),
            p if p.closed() && (self.answer_here() || self.kept_answer_here()) => self.resume(id, next),
            Paging::Stopped => {
                let count = len as u64;
                self.flash(Notice::new(Msg::ResultsPagingStopped { count }, Level::Warning));
            }
            _ => self.flash(Notice::new(Label::ResultsPageStale, Level::Warning)),
        }
    }

    /// The rows of an earlier run are shown (a later run returned none) and they are that
    /// run's last statement, on the session and binding they came from.
    pub(super) fn kept_answer_here(&self) -> bool {
        let t = self.tab();
        t.exec.kept_log.as_ref().is_some_and(|log| t.exec.shown.is_some_and(|i| i + 1 == log.len()))
            && t.exec.session.is_some()
            && t.exec.origin.is_some()
            && t.exec.origin == Self::origin_now(t)
    }

    /// `p`: the previous page, always from the fetched rows.
    pub(super) fn page_prev(&mut self) {
        let size = self.page_size.max(1);
        let t = self.tab_mut();
        let Results::Rows(rs) = &t.results else { return };
        let page = t.grid.window(rs.rows.len()).start / size;
        match page.checked_sub(1) {
            Some(p) => t.grid.set_page(p),
            None => self.flash(Notice::new(Label::ResultsPageFirst, Level::Info)),
        }
    }

    /// Fetch past the closed portal of tab `id`'s result by running its statement again and
    /// skipping the rows it has: only a statement on the allowlist, through the checks of a run,
    /// announced in the status bar and the run's Messages.
    fn resume(&mut self, id: TabId, page: usize) {
        let Some(t) = self.tabs.get(id) else { return };
        let Some(pid) = t.profile else { return };
        let sql = t.shown_sql().to_string();
        // Never held: the first page is all that was read, and the refusal says what to do.
        let released = t.exec.paging == Paging::Released;
        let refused = |why| {
            if released { Msg::ResultsPageRefusedNoHold { why } } else { Msg::ResultsPageRefused { why } }
        };
        // A table tab's filter is checked again as when it ran, and so are reads that lock rows
        // (never asked about here).
        if let Err(why) = self.table_recheck(id).and_then(|()| self.table_locks_recheck(id)) {
            return self.tab_status(id, Notice::new(refused(why), Level::Warning));
        }
        let lang = t.exec.prepared.language();
        if let Err(r) = t.exec.prepared.repeatable(&sql) {
            if released && commits_a_write(lang, &sql) {
                let count = match &t.results {
                    Results::Rows(rs) => rs.rows.len() as u64,
                    _ => 0,
                };
                let m = first_page_only(&self.i18n, lang, &sql, &r, count);
                return self.tab_status(id, Notice::new(m, Level::Warning));
            }
            let why = self.i18n.msg(&why(&r)).to_string();
            return self.tab_status(id, Notice::new(refused(why), Level::Warning));
        }
        if zone_moved(t, &sql) {
            let why = self.i18n.label(Label::RepeatZoneChanged).to_string();
            return self.tab_status(id, Notice::new(refused(why), Level::Warning));
        }
        let statements = vec![sql.clone()];
        if let Some(refused) =
            self.unsupported(id, &statements).or_else(|| self.read_only_refusal(id, pid, &statements))
        {
            return self.tab_status(id, refused);
        }
        if !self.dangerous(id, pid, &statements).is_empty() {
            let why = self.i18n.label(Label::RepeatWrites).to_string();
            return self.tab_status(id, Notice::new(refused(why), Level::Warning));
        }
        let Some(t) = self.tabs.get(id) else { return };
        let paging = self.paging_mode(t);
        let Results::Rows(rs) = &t.results else { return };
        let skip = rs.rows.len() as u64;
        // What the shown query's cap leaves past the rows held: the driver reads no row past it.
        let max_rows = self
            .table_query(id)
            .and_then(|q| q.shown.as_ref())
            .and_then(|q| q.limit)
            .map(|limit| limit.saturating_sub(skip));
        // The rows held reached the cap already: there is nothing past them to read.
        if max_rows == Some(0) {
            return;
        }
        let columns = rs.columns.iter().map(|c| (c.meta.name.clone(), c.meta.type_name.clone())).collect();
        let size = self.page_size as u64;
        // The last row this page may bring: the cap's, when it ends the page early.
        let last = max_rows.map_or(skip + size, |left| skip + left.min(size));
        let (from, to) = (datarig_i18n::fmt_count(skip + 1), datarig_i18n::fmt_count(last));
        // A table tab's order is total only when its keys end in a key the structure says is
        // unique (not known yet: not unique).
        // As the held rows' statement was sent: a structure read since does not make it total.
        let unique = match self.table_query(id) {
            Some(q) if q.shown.is_some() => q.shown_run().is_some_and(|s| s.total),
            _ => true,
        };
        let note = match (t.exec.prepared.ordered(&sql), unique, released) {
            (true, true, false) => Notice::new(Msg::ResultsPageResumed { from, to }, Level::Info),
            (true, false, false) => Notice::new(Msg::ResultsPageResumedNotUnique { from, to }, Level::Warning),
            (false, _, false) => Notice::new(Msg::ResultsPageResumedUnordered { from, to }, Level::Warning),
            (true, true, true) => Notice::new(Msg::ResultsPageRerun { from, to }, Level::Info),
            (true, false, true) => Notice::new(Msg::ResultsPageRerunNotUnique { from, to }, Level::Warning),
            (false, _, true) => Notice::new(Msg::ResultsPageRerunUnordered { from, to }, Level::Warning),
        };
        self.query_seq += 1;
        let qid = self.query_seq;
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.exec.query_id = qid;
        t.exec.resuming = Some(Resuming { columns, skip, page });
        t.exec.released = false;
        t.exec.running = Some(Running {
            id: qid,
            started: now,
            fetch: true,
            count: false,
            estimate: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: false,
        });
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
        self.send_tab(id, DbCommand::Resume { id: qid, sql, skip, paging, max_rows });
    }

    /// `#`: count the rows of the shown result, when the user asks: `SELECT count(*)` of its
    /// statement, only on the allowlist, through the checks of a run, announced; answered by
    /// `DbEvent::Counted`. A result with every row fetched is counted already. A table tab
    /// whose rows are filtered offers the planner's estimate first ([`App::open_count_menu`]).
    pub(super) fn count_rows(&mut self) {
        let Some(id) = self.count_ready() else { return };
        if self.table_query(id).and_then(|tq| tq.shown_filter()).is_some() {
            return self.open_count_menu(id);
        }
        self.count_exact();
    }

    /// The active tab's shown result may be counted or estimated now (its rows, `!more` said
    /// as counted already, nothing running, the result on the session it came from): its tab;
    /// otherwise said why (nothing for a view without rows).
    pub(super) fn count_ready(&mut self) -> Option<TabId> {
        let t = self.tab();
        let id = t.id;
        if t.exec.view != ResultView::Rows {
            return None;
        }
        let Results::Rows(rs) = &t.results else { return None };
        if !rs.more {
            let count = rs.rows.len() as u64;
            self.flash(Notice::new(Msg::ResultsCountComplete { count }, Level::Info));
            return None;
        }
        if t.exec.running.is_some() {
            self.flash_busy();
            return None;
        }
        t.profile?;
        if !(self.answer_here() || self.kept_answer_here()) {
            self.flash(Notice::new(Label::ResultsPageStale, Level::Warning));
            return None;
        }
        Some(id)
    }

    /// How long the server may count tab `id`'s rows (`[results] count_timeout`), where a
    /// limit is set: PostgreSQL only while no transaction is open on the tab's session (inside
    /// the user's, or a portal's, the count keeps to the transaction's own settings), MySQL
    /// always (for the statement alone). With `Some` when the limit is left out for an open
    /// transaction: `true` the user's own, `false` the one that holds the result's portal.
    pub(super) fn count_limit(&self, id: TabId) -> (Option<std::time::Duration>, Option<bool>) {
        let Some(t) = self.tabs.get(id) else { return (None, None) };
        let limit = self.prefs.count_timeout;
        match self.tab_dialect(id) {
            datarig_core::sql::dialect::Dialect::MySql(_) => (limit, None),
            _ if t.exec.tx_open => (None, limit.map(|_| t.exec.in_block)),
            _ => (limit, None),
        }
    }

    /// The exact count of the active tab's shown rows ([`App::count_rows`]), with its time
    /// limit ([`App::count_limit`]).
    pub(super) fn count_exact(&mut self) {
        let Some(id) = self.count_ready() else { return };
        let t = self.tab();
        let Some(pid) = t.profile else { return };
        // A table tab's filter is checked again as when it ran (never asked about here), and
        // a cap on its rows is counted within.
        if let Err(why) = self.table_recheck(id).and_then(|()| self.table_locks_recheck(id)) {
            return self.tab_status(id, Notice::new(Msg::ResultsCountRefused { why }, Level::Warning));
        }
        let source = self.table_capped_sql(id).unwrap_or_else(|| t.shown_sql().to_string());
        if zone_moved(t, &source) {
            let why = self.i18n.label(Label::RepeatZoneChanged).to_string();
            return self.tab_status(id, Notice::new(Msg::ResultsCountRefused { why }, Level::Warning));
        }
        let sql = match t.exec.prepared.count_query(&source) {
            Ok(sql) => sql,
            Err(r) => {
                let why = self.i18n.msg(&why(&r)).to_string();
                return self.tab_status(id, Notice::new(Msg::ResultsCountRefused { why }, Level::Warning));
            }
        };
        let statements = vec![sql.clone()];
        if let Some(refused) =
            self.unsupported(id, &statements).or_else(|| self.read_only_refusal(id, pid, &statements))
        {
            return self.tab_status(id, refused);
        }
        if !self.dangerous(id, pid, &statements).is_empty() {
            let why = self.i18n.label(Label::RepeatWrites).to_string();
            return self.tab_status(id, Notice::new(Msg::ResultsCountRefused { why }, Level::Warning));
        }
        let (limit, unbounded) = self.count_limit(id);
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        let qid = t.exec.query_id;
        t.exec.running = Some(Running {
            id: qid,
            started: now,
            fetch: false,
            count: true,
            estimate: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: false,
        });
        let excerpt = super::runlog::excerpt(&sql, 120);
        let m = match unbounded {
            Some(true) => Msg::ResultsCountSentUnbounded { sql: excerpt },
            Some(false) => Msg::ResultsCountSentHeld { sql: excerpt },
            None => Msg::ResultsCountSent { sql: excerpt },
        };
        let note = Notice::new(m, Level::Info);
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
        self.send_tab(id, DbCommand::Count { id: qid, sql, limit });
    }

    /// The count of tab `id`'s result `qid` came back. `snapshot`: the driver counted in a
    /// transaction with one snapshot (the server said `REPEATABLE READ` or `SERIALIZABLE`). It
    /// counts what the pages show only when that is the user's block every page was read in;
    /// otherwise it is of the rows committed when it ran, which may differ from the pages, and says so.
    pub(super) fn on_counted(
        &mut self,
        id: TabId,
        qid: u64,
        result: Result<u64, datarig_core::driver::DbError>,
        snapshot: bool,
    ) {
        let text = match &result {
            Err(e) => self.db_error_text(e),
            Ok(_) => String::new(),
        };
        let Some(t) = self.tabs.get_mut(id) else { return };
        if t.exec.query_id != qid || !t.exec.running.is_some_and(|r| r.count && !r.estimate) {
            return;
        }
        t.exec.running = None;
        let (in_block, epoch) = (t.exec.in_block, t.exec.tx_epoch);
        let note = match result {
            Ok(count) => {
                let mut now = true;
                if let Some(rs) = t.shown_rows_mut() {
                    let pages = in_block && rs.pages_in == crate::widgets::grid::PagesIn::Block(epoch);
                    now = !(snapshot && pages);
                    rs.counted = Some(count);
                    rs.counted_now = now;
                }
                let m = if now { Msg::ResultsCountDoneNow { count } } else { Msg::ResultsCountDone { count } };
                Notice::new(m, Level::Success)
            }
            Err(datarig_core::driver::DbError::Cancelled) => Notice::new(Label::ResultsCountCancelled, Level::Warning),
            // Stopped at its time limit, or behind another session's lock: the number is not
            // known (never 0).
            Err(datarig_core::driver::DbError::TimedOut(d)) => {
                Notice::new(Msg::ResultsCountTimedOut { limit: super::estimate::fmt_limit(d) }, Level::Warning)
            }
            Err(datarig_core::driver::DbError::LockTimeout(_)) => {
                Notice::new(Label::ResultsCountLockTimeout, Level::Warning)
            }
            // The server's side of the allowlist refused it (asked right before).
            Err(datarig_core::driver::DbError::NotRepeatable(r)) => {
                let why = self.i18n.msg(&why(&r)).to_string();
                Notice::new(Msg::ResultsCountRefused { why }, Level::Warning)
            }
            Err(_) => Notice::new(Msg::ResultsCountFailed { error: text }, Level::Error),
        };
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
    }
}
