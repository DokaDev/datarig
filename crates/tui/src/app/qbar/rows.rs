//! What the rows popup of a table tab's query bar says of the rows shown: the range and the
//! page of the shown (capped) result, its two page items, its two count items (the planner's
//! estimate and the exact count, each dimmed with why it would be refused now), the last
//! count with its kind, and the paging's state in full. Its items do what the bar's arrows
//! and `#` do; nothing here sends anything.

use super::super::filter_dialog::RowsItem;
use super::super::paging::Paging;
use super::Total;
use super::*;

/// An item of the rows popup: what it does, its words, and what it says beside them (`Err`:
/// it is dimmed, with why when that is known).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowsLine {
    pub(crate) item: RowsItem,
    pub(crate) text: String,
    pub(crate) note: Result<String, String>,
}

/// What the rows popup says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowsPopup {
    /// `rows 1–500 · page 1 of ~101`.
    pub(crate) title: String,
    pub(crate) items: Vec<RowsLine>,
    /// The last count and its kind.
    pub(crate) count: String,
    /// The states of the rows' paging and count, each with whether it warns.
    pub(crate) states: Vec<(String, bool)>,
}

impl App {
    /// What the rows paging says below them, when it says something (the portal closed for
    /// being idle or never held, the fetching stopped at its spill limit): the whole note, and
    /// the results title's word for the state where it does not fit.
    pub(crate) fn paging_note(&self) -> Option<(datarig_i18n::Localized, datarig_i18n::Localized)> {
        let t = self.tab();
        // The portal belongs to the last statement of the rows' run: only its rows page.
        let answer = t.exec.shown.is_some() && t.exec.shown == t.answer_index();
        // `n` is the search's while one is on, and typed while its prompt is open: the next
        // page's key there.
        let searching = self.grid_prompt_open() || self.grid_search_on();
        let next_key = self.next_page_key(if searching { Ctx::GridMatches } else { Ctx::Grid });
        let run_again = self.can_run_again();
        // A result whose portal was closed for being idle (or never held), or whose fetching
        // stopped at its spill limit, keeps its rows and says so below them.
        match &t.results {
            Results::Rows(rs)
                if answer && ((rs.more && t.exec.paging.closed()) || t.exec.paging == Paging::Stopped) =>
            {
                let count = rs.rows.len() as u64;
                let key = next_key;
                // With the results title's word for the state, where the whole note does not fit.
                let (long, short) = match t.exec.paging {
                    Paging::Stopped => (Msg::ResultsPagingStopped { count }, Label::ResultsTitlePagingStopped),
                    Paging::Released if run_again => {
                        (Msg::ResultsPagingNoHold { count, key }, Label::ResultsTitlePagingNoHold)
                    }
                    Paging::Released => (Msg::ResultsPagingFirstPageOnly { count }, Label::ResultsTitleFirstPageOnly),
                    // The next page runs the statement again when it may.
                    _ if run_again => (Msg::ResultsPagingClosedRerun { count, key }, Label::ResultsTitlePagingClosed),
                    Paging::Replaced => (Msg::ResultsPagingReplaced { count }, Label::ResultsTitlePagingClosed),
                    Paging::Interrupted => (Msg::ResultsPagingInterrupted { count }, Label::ResultsTitlePagingClosed),
                    _ => (Msg::ResultsPagingClosed { count }, Label::ResultsTitlePagingClosed),
                };
                Some((self.i18n.msg(&long), self.i18n.label(short)))
            }
            _ => None,
        }
    }

    /// Why the active tab's shown rows are neither counted nor estimated now, as `#` would say
    /// it in short ([`App::count_ready`]); `None` when they may be.
    fn rows_not_ready(&self, rs: &crate::widgets::grid::ResultSet) -> Option<String> {
        let t = self.tab();
        let label = |l: Label| Some(self.i18n.label(l).to_string());
        if !rs.more {
            return label(Label::TableRowsComplete);
        }
        if t.exec.running.is_some() {
            return label(Label::TableRowsBusy);
        }
        if t.profile.is_none() || !(self.answer_here() || self.kept_answer_here()) {
            return label(Label::TableRowsStale);
        }
        None
    }

    /// What the rows popup of the active table tab says, while it shows rows: the shown result
    /// as the bar counts it (its cap included), what each item would do now, the last count and
    /// the paging's state.
    pub(crate) fn rows_popup(&self) -> Option<RowsPopup> {
        let t = self.tab();
        if !t.is_table() {
            return None;
        }
        let Results::Rows(rs) = &t.results else { return None };
        let size = self.page_size.max(1);
        let len = rs.rows.len();
        let win = t.grid.window(len);
        let (total, estimate) = self.rows_total(t, rs);
        let bar = self.bar_rows(t, rs);
        let label = |l: Label| self.i18n.label(l).to_string();
        let pages_of = |n: u64| fmt_count(super::super::paging::pages(n, size));
        let pages = match total {
            Total::Exact(n) => pages_of(n),
            Total::About(n) => format!("~{}", pages_of(n)),
            Total::AtLeast(_) => "?".to_string(),
        };
        let (from, to) = match win.is_empty() {
            true => (0, 0),
            false => (win.start as u64 + 1, win.end.min(len) as u64),
        };
        let title = self
            .i18n
            .msg(&Msg::TableRowsTitle {
                from: fmt_count(from),
                to: fmt_count(to),
                page: fmt_count((win.start / size + 1) as u64),
                pages,
            })
            .to_string();
        // The items: the pages as the bar's arrows have them, the counts as `#` would.
        let (prev_on, next_on) = bar.arrows.unwrap_or((false, false));
        let (prev, next) = crate::icons::page_arrows(self.icons_on());
        let arrow = |on: bool| if on { Ok(String::new()) } else { Err(String::new()) };
        let ready = self.rows_not_ready(rs);
        let filtered = self.table_query(t.id).and_then(|tq| tq.shown_filter()).is_some();
        let off_text =
            |o: &crate::app::menu::Off| o.short.map_or_else(|| o.notice.render(&self.i18n).to_string(), label);
        let estimate_note = match &ready {
            Some(why) => Err(why.clone()),
            // The planner estimates filtered rows only (the whole table's: its statistics).
            None if !filtered => Err(label(Label::TableRowsUnfiltered)),
            None => match self.estimate_off(t.id) {
                Err(o) => Err(off_text(&o)),
                Ok(()) => Ok(label(Label::ResultsEstimateHint)),
            },
        };
        let exact_note = match (&ready, t.profile) {
            (Some(why), _) => Err(why.clone()),
            (None, None) => Err(label(Label::TableRowsStale)),
            (None, Some(pid)) => match self.count_sql(t.id, pid) {
                Err(o) => Err(off_text(&o)),
                Ok(_) => Ok(match self.count_limit(t.id) {
                    (Some(limit), _) => {
                        self.i18n.msg(&Msg::ResultsCountHintLimit { limit: super::super::estimate::fmt_limit(limit) })
                    }
                    (None, Some(true)) => self.i18n.label(Label::ResultsCountHintInTx),
                    (None, Some(false)) => self.i18n.label(Label::ResultsCountHintHeld),
                    (None, None) => self.i18n.label(Label::ResultsCountHintNoLimit),
                }
                .to_string()),
            },
        };
        let items = vec![
            RowsLine {
                item: RowsItem::Prev,
                text: format!("{prev} {}", label(Label::TableRowsPrev)),
                note: arrow(prev_on),
            },
            RowsLine {
                item: RowsItem::Next,
                text: format!("{next} {}", label(Label::TableRowsNext)),
                note: arrow(next_on),
            },
            RowsLine { item: RowsItem::Estimate, text: label(Label::ResultsEstimateEntry), note: estimate_note },
            RowsLine { item: RowsItem::Exact, text: label(Label::ResultsCountEntry), note: exact_note },
        ];
        // The last count, with its kind; of the shown result, within its cap.
        let rows = fmt_count;
        let count = match total {
            Total::Exact(n) if rs.more && rs.counted_now => Msg::TableRowsCountedNow { rows: rows(n) },
            Total::Exact(n) => Msg::TableRowsCounted { rows: rows(n) },
            Total::About(n) if estimate == Some(n) => Msg::TableRowsStatistics { rows: rows(n) },
            Total::About(n) => Msg::TableRowsPlanned { rows: rows(n) },
            Total::AtLeast(n) => Msg::TableRowsUncounted { rows: rows(n) },
        };
        let count = self.i18n.msg(&count).to_string();
        // What runs now, how the pages are held and what ended them.
        let mut states: Vec<(String, bool)> = Vec::new();
        if t.exec.running.is_some_and(|r| r.estimate) {
            states.push((label(Label::ResultsTitleEstimating), false));
        } else if t.exec.running.is_some_and(|r| r.count) {
            states.push((label(Label::ResultsTitleCounting), false));
        }
        match self.paging_note() {
            Some((long, _)) => states.push((long.to_string(), true)),
            None => states.extend(bar.state),
        }
        let answer = t.exec.shown.is_some() && t.exec.shown == t.answer_index();
        let cap = self.table_query(t.id).and_then(|q| q.shown.as_ref()).and_then(|q| q.limit);
        if let Some(n) = cap.filter(|&n| answer && len as u64 >= n) {
            states.push((self.i18n.msg(&Msg::ResultsTitleLimit { limit: fmt_count(n) }).to_string(), false));
        }
        Some(RowsPopup { title, items, count, states })
    }
}
