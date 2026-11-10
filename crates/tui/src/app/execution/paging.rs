//! Pages of a result: fetching the next one, read failures and idle portals.

use crate::app::*;

impl App {
    /// Ask tab `id`'s session for the next page of its result.
    pub(in crate::app) fn fetch_page(&mut self, id: TabId) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let qid = t.exec.query_id;
        t.exec.running = Some(Running {
            id: qid,
            started: Instant::now(),
            fetch: true,
            count: false,
            estimate: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: false,
        });
        self.send_tab(id, DbCommand::FetchMore { id: qid });
    }

    /// Rows of the active tab's result could not be read from its spill file (the grid shows
    /// them as not read): said in the status bar, the detail in the error log.
    pub(crate) fn results_read_failed(&mut self, fault: &datarig_core::fault::Fault) {
        // Every frame fails the same way until the rows change: said and logged once.
        if self.status.as_ref().is_some_and(|n| matches!(n.msg, Msg::ResultsReadFailed { .. })) {
            return;
        }
        let error = self.fault_text("results.read_failed", fault);
        self.status = Some(Notice::new(Msg::ResultsReadFailed { error }, Level::Error));
    }

    /// A movement key at the last row of a page that has another after it: say which key
    /// shows it (nothing is fetched by scrolling).
    pub(in crate::app) fn paging_hint(&mut self) {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return };
        let len = rs.rows.len();
        // The last page ends with the rows read (and the new rows after them).
        if !t.grid.at_page_end(len) || (t.grid.window(len).end >= len && !rs.more) {
            return;
        }
        // `n` is the search's while one is on: the next page's key there.
        let key = self.next_page_key(self.grid_ctx());
        self.flash(Notice::new(Msg::ResultsPageHint { key }, Level::Info));
    }

    /// Tab `id`'s result has had its portal open for the policy's `paging_idle_timeout`
    /// without a fetch: ask the driver to close it (it ends the implicit transaction that held
    /// it, never the user's own). The rows stay, and the server may have more (unknown is not
    /// absent): the next page runs the statement again when it is on the allowlist.
    pub(in crate::app) fn close_idle_portal(&mut self, id: TabId) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let Some((rs, _)) = t.answer_rows_mut() else { return };
        let count = rs.rows.len() as u64;
        t.exec.paging = Paging::ClosedIdle;
        let rerun = t.exec.prepared.repeatable(t.answer_sql()).is_ok();
        t.exec.rerun_ok = rerun;
        let qid = t.exec.query_id;
        self.send_tab(id, DbCommand::ClosePortal { id: qid });
        let m = if rerun {
            // `n` is the search's in a tab whose search is on (shown or not), and typed while its
            // prompt is open.
            let on = self.tabs.get(id).is_some_and(|t| t.find.prompting() || crate::app::grid_search::search_on(t));
            let key = self.next_page_key(if on { Ctx::GridMatches } else { Ctx::Grid });
            Msg::ResultsPagingClosedRerun { count, key }
        } else {
            Msg::ResultsPagingClosed { count }
        };
        self.tab_status(id, Notice::new(m, Level::Info));
    }
}
