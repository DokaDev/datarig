//! The planner's estimate of a filtered table tab's rows, on request only: `#` on a table tab
//! whose rows are filtered opens a small choice, the planner's estimate or the exact count. The
//! estimate is the server's `EXPLAIN` of the statement the rows were read with (never
//! `ANALYZE`), sent on the tab's session through the same checks as a count, only while no
//! transaction is open there, and for a filter the planner cannot run anything for: no call
//! outside the filter allowlist, and on MySQL no subquery. On PostgreSQL the server is asked
//! whether planning the relation may run code of its own; a refusal stays with the rows, and the
//! entry is dimmed with it from then on. Its answer is bound to the rows shown (the result's
//! query id): a late answer of rows replaced since is dropped.

use super::menu::{MenuItem, MenuTarget, Off};
use super::tabs::ResultView;
use super::*;
use datarig_core::driver::DbError;
use datarig_core::query::{self, EstimateOff};
use std::time::Duration;

/// An entry of the choice `#` opens on a filtered table tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CountChoice {
    /// The planner's estimate (`EXPLAIN`, no row read).
    Estimate,
    /// The exact count (`SELECT count(*)`, with its time limit).
    Exact,
}

/// A time limit as the UI says it: `30s`, `2m` in whole units, else in milliseconds.
pub(crate) fn fmt_limit(d: Duration) -> String {
    if d.subsec_millis() == 0 && d.as_secs() > 0 {
        datarig_core::policy::format_timeout(Some(d))
    } else {
        format!("{}ms", d.as_millis())
    }
}

impl App {
    /// Why table tab `id`'s shown rows get no planner estimate now, in the words a menu shows
    /// and those its pick says; `Ok` when they may.
    pub(super) fn estimate_off(&self, id: TabId) -> Result<(), Box<Off>> {
        let off = |short: Label, long: Label| Err(Box::new(Off::new(short, Notice::new(long, Level::Warning))));
        let Some(t) = self.tabs.get(id) else {
            return off(Label::ResultsEstimateOffServer, Label::ResultsEstimateServer);
        };
        if self.tab_explain(id).is_none() {
            return off(Label::ResultsEstimateOffServer, Label::ResultsEstimateServer);
        }
        if t.exec.tx_open {
            return off(Label::ResultsEstimateOffTx, Label::ResultsEstimateTx);
        }
        let Some(text) = self.table_query(id).and_then(|tq| tq.shown_filter()) else {
            return off(Label::ResultsEstimateOffCalls, Label::ResultsEstimateCalls);
        };
        // A filter refused now (or calling functions not let run in the tab) is not planned.
        let refused = |why: String| {
            let notice = Notice::new(Msg::ResultsEstimateRefused { why }, Level::Warning);
            Box::new(Off::new(Label::ResultsEstimateOffRefused, notice))
        };
        // The server already said these rows' relation is not to be planned.
        if let Results::Rows(rs) = &t.results
            && let Some(why) = &rs.planner_off
        {
            return Err(refused(why.clone()));
        }
        self.table_recheck(id).map_err(refused)?;
        let (Some(name), Some(target)) = (self.table_name(id), self.table_target(id)) else {
            return off(Label::ResultsEstimateOffCalls, Label::ResultsEstimateCalls);
        };
        let read_only = t.profile.is_some_and(|p| self.read_only(p));
        let Ok(verdict) = query::check_filter(text, &name, &target, &self.classifier(id), read_only) else {
            return off(Label::ResultsEstimateOffCalls, Label::ResultsEstimateCalls);
        };
        match query::estimate_allowed(&verdict, text, self.tab_dialect(id)) {
            Ok(()) => Ok(()),
            Err(EstimateOff::Calls) => off(Label::ResultsEstimateOffCalls, Label::ResultsEstimateCalls),
            Err(EstimateOff::Subquery) => off(Label::ResultsEstimateOffSubquery, Label::ResultsEstimateSubquery),
        }
    }

    /// The choice `#` opens on table tab `id` with filtered rows: the planner's estimate (dimmed
    /// with why when there is none to ask for) and the exact count, with what each does.
    pub(super) fn open_count_menu(&mut self, id: TabId) {
        if self.focus != Focus::Inspector {
            self.focus = Focus::Results;
        }
        let (Some(grid), Some(result)) = (self.menu_grid_target(), self.grid_result_id()) else { return };
        let estimate = MenuItem::Count(CountChoice::Estimate);
        let exact = MenuItem::Count(CountChoice::Exact);
        let mut off = Vec::new();
        let mut hints = vec![(estimate, self.i18n.label(Label::ResultsEstimateHint))];
        if let Err(o) = self.estimate_off(id) {
            off.push((estimate, *o));
            hints.clear();
        }
        let exact_hint = match self.count_limit(id) {
            (Some(limit), _) => self.i18n.msg(&Msg::ResultsCountHintLimit { limit: fmt_limit(limit) }),
            (None, Some(true)) => self.i18n.label(Label::ResultsCountHintInTx),
            (None, Some(false)) => self.i18n.label(Label::ResultsCountHintHeld),
            (None, None) => self.i18n.label(Label::ResultsCountHintNoLimit),
        };
        hints.push((exact, exact_hint));
        let names = vec![
            (estimate, self.i18n.label(Label::ResultsEstimateEntry)),
            (exact, self.i18n.label(Label::ResultsCountEntry)),
        ];
        let title = self.i18n.label(Label::ResultsCountMenuTitle).to_string();
        let at = self.menu_at_cell();
        let target = MenuTarget::Filter { grid: Box::new(grid), result };
        self.push_filter_menu(target, title, (vec![estimate, exact], off, names, hints, Vec::new()), at);
    }

    /// An entry of the count choice was picked (on the rows it was opened on still).
    pub(super) fn count_choice(&mut self, choice: CountChoice) {
        match choice {
            CountChoice::Estimate => self.estimate_rows(),
            CountChoice::Exact => self.count_exact(),
        }
    }

    /// The planner's estimate of the active table tab's filtered rows: the statement they were
    /// read with (within its cap), checked again as a count is, sent as
    /// [`DbCommand::Estimate`]; answered by `DbEvent::Estimated`.
    pub(super) fn estimate_rows(&mut self) {
        let Some(id) = self.count_ready() else { return };
        let t = self.tab();
        let Some(pid) = t.profile else { return };
        if t.exec.view != ResultView::Rows || self.table_query(id).and_then(|tq| tq.shown_filter()).is_none() {
            return;
        }
        if let Err(o) = self.estimate_off(id) {
            return self.tab_status(id, o.notice);
        }
        if let Err(why) = self.table_recheck(id).and_then(|()| self.table_locks_recheck(id)) {
            return self.tab_status(id, Notice::new(Msg::ResultsEstimateRefused { why }, Level::Warning));
        }
        let sql = self.table_capped_sql(id).unwrap_or_else(|| t.shown_sql().to_string());
        let statements = vec![sql.clone()];
        if let Some(refused) =
            self.unsupported(id, &statements).or_else(|| self.read_only_refusal(id, pid, &statements))
        {
            return self.tab_status(id, refused);
        }
        if !self.dangerous(id, pid, &statements).is_empty() {
            let why = self.i18n.label(Label::RepeatWrites).to_string();
            return self.tab_status(id, Notice::new(Msg::ResultsEstimateRefused { why }, Level::Warning));
        }
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        let qid = t.exec.query_id;
        // A count as far as the tab goes (not a run: the rows stay), told apart by `estimate`.
        t.exec.running = Some(Running {
            id: qid,
            started: now,
            fetch: false,
            count: true,
            estimate: true,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: false,
        });
        let note = Notice::new(Msg::ResultsEstimateSent { sql: super::runlog::excerpt(&sql, 120) }, Level::Info);
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
        self.send_tab(id, DbCommand::Estimate { id: qid, sql });
    }

    /// The estimate of tab `id`'s result `qid` came back: kept with the rows it is of, or why
    /// there is none (never a count of 0). An answer of rows replaced since is dropped.
    pub(super) fn on_estimated(&mut self, id: TabId, qid: u64, result: Result<u64, DbError>) {
        let text = match &result {
            Err(e) => self.db_error_text(e),
            Ok(_) => String::new(),
        };
        let Some(t) = self.tabs.get_mut(id) else { return };
        if t.exec.query_id != qid || !t.exec.running.is_some_and(|r| r.estimate) {
            return;
        }
        t.exec.running = None;
        let note = match result {
            Ok(count) => {
                if let Some(rs) = t.shown_rows_mut() {
                    rs.planner = Some(count);
                }
                Notice::new(Msg::ResultsEstimateDone { count }, Level::Success)
            }
            Err(DbError::Cancelled) => Notice::new(Label::ResultsEstimateCancelled, Level::Warning),
            Err(DbError::TimedOut(d)) => {
                Notice::new(Msg::ResultsEstimateTimedOut { limit: fmt_limit(d) }, Level::Warning)
            }
            Err(DbError::LockTimeout(_)) => Notice::new(Label::ResultsEstimateLockTimeout, Level::Warning),
            // The session's own refusal: a transaction opened meanwhile, or no plan format.
            Err(DbError::NotSupported) => Notice::new(Label::ResultsEstimateNotSent, Level::Warning),
            Err(DbError::NotRepeatable(r)) => {
                let why = self.i18n.msg(&super::pages::why(&r)).to_string();
                if let Some(rs) = t.shown_rows_mut() {
                    rs.planner_off = Some(why.clone());
                }
                Notice::new(Msg::ResultsEstimateRefused { why }, Level::Warning)
            }
            Err(DbError::NoResult) => Notice::new(Label::ResultsEstimateNone, Level::Warning),
            Err(_) => Notice::new(Msg::ResultsEstimateFailed { error: text }, Level::Error),
        };
        t.exec.run.notes.push(note.clone());
        self.tab_status(id, note);
    }
}
