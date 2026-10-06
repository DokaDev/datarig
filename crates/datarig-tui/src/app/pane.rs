//! The panes of the active tab: a query tab shows its editor and, once it
//! ran something, its results pane below it (resized or hidden per tab); a table tab
//! shows its results at full height. Which panes are drawn decides where the focus may be.
//!
//! One pane can be zoomed to the whole workspace, as tmux does (per tab; the results'
//! "maximise" is their zoom): the others are not drawn, and moving the focus to one of them
//! ends the zoom. The explorer can be hidden and resized (for every tab).

use super::action::PanelAction;
use super::tabs::PaneLayout;
use super::*;

/// The fewest lines the editor pane keeps next to the results (borders included).
pub const MIN_EDITOR_ROWS: u16 = 4;
/// The fewest lines the results pane keeps next to the editor (borders included).
pub const MIN_RESULTS_ROWS: u16 = 6;

/// The narrowest the explorer gets by its keys or a drag (its borders included).
pub const EXPLORER_MIN: u16 = 16;
/// One key press widens or narrows the explorer by this many columns.
pub const EXPLORER_STEP: u16 = 4;

/// The widest the explorer gets in a workspace `total` columns wide: three fifths of it.
pub fn explorer_max(total: u16) -> u16 {
    (u32::from(total) * 3 / 5) as u16
}

/// The explorer's width in a workspace `total` columns wide: the user's `width` within
/// [`EXPLORER_MIN`] and [`explorer_max`] (a narrower terminal draws it narrower and the
/// preference stays), else a quarter of the width, 24 to 40 columns.
pub fn explorer_width(total: u16, width: Option<u16>) -> u16 {
    let max = explorer_max(total);
    match width {
        Some(w) => w.clamp(EXPLORER_MIN.min(max), max),
        None => (u32::from(total) * 25 / 100).clamp(24, 40) as u16,
    }
}

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
    /// something (or while launch notices wait to be read there), unless the user hid it; a DDL
    /// tab never (it has no results).
    pub fn results_shown(&self) -> bool {
        if self.tabs.is_empty() || self.profiles.is_empty() || self.tab().is_ddl() {
            return false;
        }
        let t = self.tab();
        t.is_table() || (!t.pane.hidden && (t.ran || !matches!(t.results, Results::Empty) || !self.notices.is_empty()))
    }

    /// The active tab has an editor: a query or DDL tab (a zoom of another pane may keep it
    /// off screen: see [`App::zoomed`]).
    pub fn editor_shown(&self) -> bool {
        !(self.tabs.is_empty() || self.tab().is_table())
    }

    /// The pane zoomed to the whole workspace now: the active tab's zoom while that pane is
    /// there (a zoom of the results waits for them, as their maximise did).
    pub fn zoomed(&self) -> Option<Focus> {
        if self.tabs.is_empty() || self.profiles.is_empty() {
            return None;
        }
        self.tab().pane.zoom.filter(|z| self.focusable(*z))
    }

    /// The explorer is drawn when nothing is zoomed: the user did not hide it, or there is no
    /// tab (then it is all there is).
    pub fn explorer_shown(&self) -> bool {
        !self.explorer.hidden || self.tabs.is_empty() || self.profiles.is_empty()
    }

    /// Whether `f` is a pane of the active tab (or the explorer) that can have the focus. A
    /// zoom or a hidden explorer keeps it off screen until it has the focus.
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
            // The results' zoom.
            PanelAction::Maximize if t.pane.zoom == Some(Focus::Results) => t.pane.zoom = None,
            PanelAction::Maximize => {
                t.pane.zoom = Some(Focus::Results);
                if !matches!(self.focus, Focus::Results | Focus::Inspector) {
                    self.focus = Focus::Results;
                }
            }
            PanelAction::Grow => t.pane.share = PaneLayout::clamped(t.pane.share + PaneLayout::STEP),
            PanelAction::Shrink => t.pane.share = PaneLayout::clamped(t.pane.share.saturating_sub(PaneLayout::STEP)),
        }
        self.fix_focus();
        self.mark_workspace();
    }

    /// `pane.zoom`: the focused pane takes the whole workspace, or the zoom ends.
    pub(super) fn toggle_zoom(&mut self) {
        let zoom = match self.zoomed() {
            Some(_) => None,
            None => Some(self.focus),
        };
        self.tab_mut().pane.zoom = zoom;
        self.mark_workspace();
    }

    /// After every input, `moved`: the input moved the focus. A move to a pane the zoom
    /// hides ends the zoom (as tmux does); otherwise (another tab became active, the zoomed
    /// pane came back) the focus goes to the zoomed pane. The zoom of a pane that went away
    /// ends, but the results' waits for them, as their maximise did.
    pub(super) fn fix_zoom(&mut self, moved: bool) {
        // The action menu moves the focus to the pane whose actions it lists while it is open
        // (the grid's, from the inspector): not a move of the user's.
        if self.tabs.is_empty() || self.profiles.is_empty() || self.overlays.is_open(OverlayKind::ContextMenu) {
            return;
        }
        let Some(z) = self.tab().pane.zoom else { return };
        if !self.focusable(z) {
            if z != Focus::Results {
                self.tab_mut().pane.zoom = None;
            }
            return;
        }
        if self.focus == z || (z == Focus::Results && self.focus == Focus::Inspector) {
            return;
        }
        if moved {
            self.tab_mut().pane.zoom = None;
            self.mark_workspace();
        } else {
            self.focus = z;
        }
    }

    /// The focus is on the hidden explorer and nothing zooms it: moved there (or with nothing
    /// else to focus), it is shown again; otherwise (another tab became active, a zoom of it
    /// ended) the focus goes to the editor or the results and it stays hidden.
    pub(super) fn fix_explorer(&mut self, moved: bool) {
        if self.focus != Focus::Tree || !self.explorer.hidden || self.zoomed() == Some(Focus::Tree) {
            return;
        }
        let other = [Focus::Editor, Focus::Results].into_iter().find(|f| self.focusable(*f));
        match other {
            Some(f) if !moved && !self.tabs.is_empty() && !self.profiles.is_empty() => self.focus = f,
            _ => {
                self.explorer.hidden = false;
                self.mark_workspace();
            }
        }
    }

    /// Keep the focus, the zoom and the explorer together (`moved`: the focus was moved by the
    /// user): after every input, and after the events that change what the panes show.
    pub(super) fn settle_panes(&mut self, moved: bool) {
        self.fix_focus();
        self.fix_zoom(moved);
        self.fix_explorer(moved);
    }

    /// `explorer.toggle`: hide the explorer (the focus leaves it for the editor or the results)
    /// or show it again (a zoom of another pane ends).
    pub(super) fn toggle_explorer(&mut self) {
        let shown = match self.zoomed() {
            Some(z) => z == Focus::Tree,
            None => !self.explorer.hidden,
        };
        if shown {
            self.explorer.hidden = true;
            if self.focus == Focus::Tree {
                self.explorer.filtering = false;
                self.focus =
                    [Focus::Editor, Focus::Results].into_iter().find(|f| self.focusable(*f)).unwrap_or(Focus::Tree);
            }
        } else {
            self.explorer.hidden = false;
            if self.zoomed().is_some() {
                self.tab_mut().pane.zoom = None;
            }
        }
        self.mark_workspace();
    }

    /// `explorer.wider` / `explorer.narrower`: the explorer by a step, within its limits.
    pub(super) fn step_explorer(&mut self, wider: bool) {
        let now = explorer_width(self.layout.workspace.width, self.explorer.width);
        self.resize_explorer(if wider { now + EXPLORER_STEP } else { now.saturating_sub(EXPLORER_STEP) });
    }

    /// The explorer is to be `width` columns wide (a key, a drag of its border): kept within its
    /// limits for the workspace as drawn.
    pub(super) fn resize_explorer(&mut self, width: u16) {
        let total = self.layout.workspace.width;
        if total == 0 {
            return;
        }
        let w = explorer_width(total, Some(width));
        // Nothing changes on screen (a press on the border, a key at a limit): the preference
        // stays as it is, a wider one or none (the default, following the terminal's width).
        if w == explorer_width(total, self.explorer.width) {
            return;
        }
        self.explorer.width = Some(w);
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
            // A plan: its rows, the plan and the Messages to switch between.
            || t.exec.plan.is_some()
            // A chart: the rows and the chart.
            || (t.exec.chart.is_some() && rows)
    }

    /// Show the result tab `delta` places away (the row results of the run in their order,
    /// the chart of the shown one, its plan when it has one, then its Messages), wrapping
    /// around.
    pub(super) fn cycle_result_tab(&mut self, delta: isize) {
        use super::tabs::ResultView;
        let t = self.tab_mut();
        let mut views: Vec<(ResultView, Option<usize>)> =
            t.result_tabs().into_iter().map(|i| (ResultView::Rows, Some(i))).collect();
        if t.exec.chart.is_some() && matches!(t.results, Results::Rows(_)) {
            views.push((ResultView::Chart, None));
        }
        if t.exec.plan.is_some() {
            views.push((ResultView::Plan, None));
        }
        views.push((ResultView::Messages, None));
        let n = views.len() as isize;
        let now = match (t.exec.view, t.exec.shown) {
            (ResultView::Rows, Some(i)) => views.iter().position(|v| *v == (ResultView::Rows, Some(i))),
            (view, _) => views.iter().position(|v| v.0 == view),
        };
        let next = (now.unwrap_or(0) as isize + delta).rem_euclid(n) as usize;
        match views[next] {
            (ResultView::Rows, Some(i)) => t.show_result(i),
            (view, _) => t.exec.view = view,
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
        if t.is_query() {
            t.pane.share = PaneLayout::clamped(share);
            self.mark_workspace();
        }
    }
}

#[cfg(test)]
mod tests;
