//! The panes of the active tab: a query tab shows its editor and, once it
//! ran something, its results pane below it (resized, hidden or maximised per tab); a table tab
//! shows its results at full height. Which panes are drawn decides where the focus may be.

use super::action::PanelAction;
use super::tabs::PaneLayout;
use super::*;

/// The fewest lines the editor pane keeps next to the results (borders included).
pub const MIN_EDITOR_ROWS: u16 = 4;
/// The fewest lines the results pane keeps next to the editor (borders included).
pub const MIN_RESULTS_ROWS: u16 = 6;

/// How the editor and results split `height` lines at `share` percent for the results: the
/// results' height, within the minimums of both panes (half each when both do not fit).
pub fn results_rows(height: u16, share: u16) -> u16 {
    if height < MIN_EDITOR_ROWS + MIN_RESULTS_ROWS {
        return height / 2;
    }
    let want = ((u32::from(height) * u32::from(share) + 50) / 100) as u16;
    want.clamp(MIN_RESULTS_ROWS, height - MIN_EDITOR_ROWS)
}

impl App {
    /// The active tab shows a results pane: a table tab always; a query tab once it ran
    /// something (or while launch notices wait to be read there), unless the user hid it.
    pub fn results_shown(&self) -> bool {
        if self.tabs.is_empty() || self.profiles.is_empty() {
            return false;
        }
        let t = self.tab();
        t.is_table() || (!t.pane.hidden && (t.ran || !matches!(t.results, Results::Empty) || !self.notices.is_empty()))
    }

    /// The active tab shows its editor: a query tab whose results are not maximised.
    pub fn editor_shown(&self) -> bool {
        let maximized = self.results_shown() && self.tab().pane.maximized;
        !(self.tabs.is_empty() || self.tab().is_table() || maximized)
    }

    /// Whether `f` is a pane on screen that can have the focus.
    pub fn focusable(&self, f: Focus) -> bool {
        if self.profiles.is_empty() {
            // The welcome panel stands in the editor's place.
            return matches!(f, Focus::Tree | Focus::Editor);
        }
        match f {
            Focus::Tree => true,
            Focus::Editor => self.editor_shown(),
            Focus::Results => self.results_shown(),
            Focus::Inspector => self.results_shown() && self.inspector_shown(),
        }
    }

    /// Keep the focus on a pane that is drawn: the results, else the editor, else the
    /// explorer.
    pub(super) fn fix_focus(&mut self) {
        if self.focusable(self.focus) {
            return;
        }
        self.focus = [Focus::Results, Focus::Editor, Focus::Tree]
            .into_iter()
            .find(|f| self.focusable(*f))
            .unwrap_or(Focus::Tree);
    }

    /// What `a` does to the active query tab's results pane.
    pub(super) fn panel_action(&mut self, a: PanelAction) {
        let t = self.tab_mut();
        match a {
            PanelAction::Toggle => {
                t.pane.hidden = !t.pane.hidden;
                if t.pane.hidden && matches!(self.focus, Focus::Results | Focus::Inspector) {
                    self.focus = Focus::Editor;
                }
            }
            PanelAction::Maximize => {
                t.pane.maximized = !t.pane.maximized;
                if t.pane.maximized && self.focus == Focus::Editor {
                    self.focus = Focus::Results;
                }
            }
            PanelAction::Grow => t.pane.share = PaneLayout::clamped(t.pane.share + PaneLayout::STEP),
            PanelAction::Shrink => t.pane.share = PaneLayout::clamped(t.pane.share.saturating_sub(PaneLayout::STEP)),
        }
        self.fix_focus();
        self.mark_workspace();
    }

    /// The results pane of the active query tab has its result tab strip: when there is more
    /// than the one result of one statement to switch to (a run of several statements, rows of
    /// an earlier run, the Messages chosen next to rows) or something to say about the rows
    /// (they were read inside the user's transaction). A single statement's result otherwise
    /// looks as before.
    pub fn strip_shown(&self) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let t = self.tab();
        if t.is_table() || !t.ran {
            return false;
        }
        let rows = !t.result_tabs().is_empty();
        let tx =
            matches!(&t.results, Results::Rows(rs) if rs.tx.is_some() && t.exec.view == super::tabs::ResultView::Rows);
        tx || t.exec.run.several()
            || t.rows_log().several()
            || t.exec.kept_log.is_some()
            || (rows && t.exec.view == super::tabs::ResultView::Messages)
            || t.result_tabs().len() > 1
    }

    /// Show the result tab `delta` places away (the row results of the run in their order,
    /// then its Messages), wrapping around.
    pub(super) fn cycle_result_tab(&mut self, delta: isize) {
        let t = self.tab_mut();
        let tabs = t.result_tabs();
        let n = tabs.len() as isize + 1;
        let now = match (t.exec.view, t.exec.shown) {
            (super::tabs::ResultView::Rows, Some(i)) => tabs.iter().position(|x| *x == i).unwrap_or(0) as isize,
            _ => n - 1,
        };
        let next = (now + delta).rem_euclid(n) as usize;
        match tabs.get(next) {
            Some(&i) => t.show_result(i),
            None => t.exec.view = super::tabs::ResultView::Messages,
        }
    }

    /// The divider (the results pane's top border) was dragged to line `y`: the results take
    /// the lines from there down, within the limits.
    pub(super) fn drag_divider(&mut self, y: u16) {
        let body = self.layout.body;
        if body.height == 0 {
            return;
        }
        let bottom = body.y + body.height;
        let rows = bottom.saturating_sub(y.max(body.y));
        let share = (u32::from(rows) * 100 / u32::from(body.height)) as u16;
        let t = self.tab_mut();
        if !t.is_table() {
            t.pane.share = PaneLayout::clamped(share);
            self.mark_workspace();
        }
    }
}

#[cfg(test)]
mod tests;
