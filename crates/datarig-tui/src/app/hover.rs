//! The pointer over the workspace's small clickable targets (no dialog open): the document tab
//! bar's tabs, their `×` and scroll marks, the result tab strip, the paging arrows and the Plan
//! tab's view names. The one
//! under the pointer is drawn highlighted; it is found in what the last frame drew (the same
//! hits a click uses), and a frame that lays those targets out differently drops it, so no
//! highlight is left on something else.

use super::*;
use crate::widgets::tabbar::TabHit;
use ratatui::layout::Position;

/// A small clickable target of the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerOn {
    /// A tab of the document tab bar, its `×` or a scroll mark.
    Tab(TabHit),
    /// A result tab of the strip: its statement (`None`: the Messages or Plan view).
    Strip(Option<usize>, tabs::ResultView),
    /// The results' previous and next page arrows.
    PagePrev,
    PageNext,
    /// A view's name on the Plan tab's first line.
    PlanView(super::plan::PlanView),
    /// A kind or a column choice on the Chart tab's first lines.
    Chart(super::chart::ChartHit),
}

impl App {
    /// Every frame, also one too small to draw: a dialog that came on top since the last one
    /// (opened, or uncovered by one that closed) starts its arming clock over, even when no
    /// frame showed it covered.
    pub fn note_top(&mut self) {
        let top = self.overlays.top().map(|o| o.kind());
        if std::mem::replace(&mut self.last_top, top) != top {
            self.overlays.rearm_top();
        }
    }

    /// The target drawn highlighted: none while a dialog is open.
    pub(crate) fn pointer_hover(&self) -> Option<PointerOn> {
        self.pointer_on.filter(|_| !self.modal_open())
    }

    /// The pointer moved over the workspace: `true` when another target (or none) is under it
    /// now, so the frame changes.
    pub(super) fn workspace_hover(&mut self, m: MouseEvent) -> bool {
        let (x, l) = (m.column, self.layout);
        let at = Position::new(m.column, m.row);
        let on = if l.too_small {
            None
        } else if l.tab_bar.contains(at) {
            self.tab_hits.iter().find(|(a, b, _)| x >= *a && x < *b).map(|h| PointerOn::Tab(h.2))
        } else if l.strip.contains(at) {
            self.strip_hits.iter().find(|(a, b, ..)| x >= *a && x < *b).map(|h| PointerOn::Strip(h.2, h.3))
        } else if !self.tabs.is_empty() && l.page_prev.contains(at) {
            Some(PointerOn::PagePrev)
        } else if !self.tabs.is_empty() && l.page_next.contains(at) {
            Some(PointerOn::PageNext)
        } else if !self.tabs.is_empty() && self.plan_shown() && l.results.contains(at) {
            let plan = self.tab().exec.plan.as_ref();
            plan.and_then(|p| p.view_hits.iter().find(|(r, _)| r.contains(at))).map(|h| PointerOn::PlanView(h.1))
        } else if !self.tabs.is_empty() && self.chart_shown() && l.results.contains(at) {
            let chart = self.tab().exec.chart.as_ref();
            chart.and_then(|c| c.field_hits.iter().find(|(r, _)| r.contains(at))).map(|h| PointerOn::Chart(h.1))
        } else {
            None
        };
        std::mem::replace(&mut self.pointer_on, on) != on
    }
}
