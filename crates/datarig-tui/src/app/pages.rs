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
use datarig_core::sql::risk::repeat::{self, NotRepeatable};

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
        NotRepeatable::Unreadable => Label::RepeatUnreadable.into(),
    }
}

/// Why a result not held shows `count` rows only (its statement `sql` is not run again for the
/// user, for `r`): a statement that changes rows ran to its end and is committed (running it
/// again would change rows again), anything else stopped after its first page.
pub(super) fn first_page_only(i18n: &I18n, sql: &str, r: &NotRepeatable, count: u64) -> Msg {
    if datarig_core::sql::risk::classify(sql).writes {
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
    fn answer_here(&self) -> bool {
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
            return self.flash(Notice::new(Label::ResultsPageLast, Level::Info));
        }
        let (id, paging) = (t.id, t.exec.paging);
        match paging {
            Paging::Open { .. } if self.answer_here() => {
                self.tab_mut().exec.want_page = Some(next);
                self.fetch_page(id);
            }
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
    fn kept_answer_here(&self) -> bool {
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
        if let Err(r) = repeat::repeatable(&sql) {
            if released && datarig_core::sql::risk::classify(&sql).writes {
                let count = match &t.results {
                    Results::Rows(rs) => rs.rows.len() as u64,
                    _ => 0,
                };
                let m = first_page_only(&self.i18n, &sql, &r, count);
                return self.tab_status(id, Notice::new(m, Level::Warning));
            }
            let why = self.i18n.msg(&why(&r)).to_string();
            return self.tab_status(id, Notice::new(refused(why), Level::Warning));
        }
        let statements = vec![sql.clone()];
        if let Some(refused) = self.unsupported(&statements).or_else(|| self.read_only_refusal(id, pid, &statements)) {
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
        let columns = rs.columns.iter().map(|c| (c.meta.name.clone(), c.meta.type_name.clone())).collect();
        let size = self.page_size as u64;
        let (from, to) = (datarig_core::i18n::fmt_count(skip + 1), datarig_core::i18n::fmt_count(skip + size));
        let note = match (repeat::ordered(&sql), released) {
            (true, false) => Notice::new(Msg::ResultsPageResumed { from, to }, Level::Info),
            (false, false) => Notice::new(Msg::ResultsPageResumedUnordered { from, to }, Level::Warning),
            (true, true) => Notice::new(Msg::ResultsPageRerun { from, to }, Level::Info),
            (false, true) => Notice::new(Msg::ResultsPageRerunUnordered { from, to }, Level::Warning),
        };
        self.query_seq += 1;
        let qid = self.query_seq;
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.exec.query_id = qid;
        t.exec.resuming = Some(Resuming { columns, skip, page });
        t.exec.released = false;
        t.exec.running = Some(Running { id: qid, started: now, fetch: true, count: false, cancelling: None });
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
        self.send_tab(id, DbCommand::Resume { id: qid, sql, skip, paging });
    }

    /// `#`: count the rows of the shown result, when the user asks: `SELECT count(*)` of its
    /// statement, only on the allowlist, through the checks of a run, announced; answered by
    /// `DbEvent::Counted`. A result with every row fetched is counted already.
    pub(super) fn count_rows(&mut self) {
        let t = self.tab();
        let id = t.id;
        if t.exec.view != ResultView::Rows {
            return;
        }
        let Results::Rows(rs) = &t.results else { return };
        if !rs.more {
            let count = rs.rows.len() as u64;
            return self.flash(Notice::new(Msg::ResultsCountComplete { count }, Level::Info));
        }
        if t.exec.running.is_some() {
            return self.flash_busy();
        }
        let Some(pid) = t.profile else { return };
        if !(self.answer_here() || self.kept_answer_here()) {
            return self.flash(Notice::new(Label::ResultsPageStale, Level::Warning));
        }
        let sql = match repeat::count_query(t.shown_sql()) {
            Ok(sql) => sql,
            Err(r) => {
                let why = self.i18n.msg(&why(&r)).to_string();
                return self.tab_status(id, Notice::new(Msg::ResultsCountRefused { why }, Level::Warning));
            }
        };
        let statements = vec![sql.clone()];
        if let Some(refused) = self.unsupported(&statements).or_else(|| self.read_only_refusal(id, pid, &statements)) {
            return self.tab_status(id, refused);
        }
        if !self.dangerous(id, pid, &statements).is_empty() {
            let why = self.i18n.label(Label::RepeatWrites).to_string();
            return self.tab_status(id, Notice::new(Msg::ResultsCountRefused { why }, Level::Warning));
        }
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        let qid = t.exec.query_id;
        t.exec.running = Some(Running { id: qid, started: now, fetch: false, count: true, cancelling: None });
        let note = Notice::new(Msg::ResultsCountSent { sql: super::runlog::excerpt(&sql, 120) }, Level::Info);
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
        self.send_tab(id, DbCommand::Count { id: qid, sql });
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
        if t.exec.query_id != qid || !t.exec.running.is_some_and(|r| r.count) {
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
