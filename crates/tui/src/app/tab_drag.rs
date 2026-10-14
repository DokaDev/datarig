//! Moving tabs in the tab bar: by the mouse (press a tab, drag it sideways, release where it
//! goes) and by keys (`tab.move_left` / `tab.move_right`, `:tabmove`).
//!
//! A press on a tab makes it active, as a click does, and arms a drag. Only once the pointer
//! went [`LIFT_COLUMNS`] columns sideways is the tab lifted: drawn in the theme's selection
//! style, with a marker where it would land (none while it would stay where it is); before
//! that a release is the click it was. Over a tab, the marker goes before or after it by which
//! half the pointer is on; over a scroll mark (`‹` / `›`) it goes past the hidden tab on that
//! side, so the bar scrolls by one tab each time the pointer moves there; left or right of the
//! bar's tabs, the nearest end. More than [`OUTSIDE_ROWS`] rows above or below the bar the
//! tab has nowhere to land (no marker) and a release there drops nothing; back near the bar,
//! the drag goes on. `Esc`, a dialog coming on top, a press of another button or a lost
//! release end the drag where it is: nothing moves.
//!
//! A tab's number in the bar is its place, so the numbers (and `Space 1`..`9`) follow the new
//! order; a console's own name (`console 3`) does not change. The new order is written to the
//! workspace state at once.

use super::*;
use crate::widgets::tabbar::TabHit;

/// How far sideways (in columns) the pointer goes from the press before the tab is lifted.
pub const LIFT_COLUMNS: u16 = 2;

/// How many rows above or below the tab bar a lifted tab still has a place to land.
pub const OUTSIDE_ROWS: u16 = 2;

/// A press on a tab of the tab bar, and the drag that may follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabDrag {
    /// The pressed tab (made active by the press).
    pub tab: TabId,
    /// The column the press was at.
    pub press_x: u16,
    /// The pointer went far enough sideways: the tab is being dragged.
    pub lifted: bool,
    /// Where the tab would land, as a place between tabs (`0`: before the first, the number
    /// of tabs: after the last); `None` while the pointer is too far from the bar.
    pub slot: Option<usize>,
}

/// Where a tab dragged to column `x` lands, as a place between tabs, by what the tab bar drew
/// (`hits`, left to right): before a tab under the left half of it, after it under the right
/// half; over a scroll mark, past the hidden tab on its side; left or right of everything
/// drawn, as over the first or last thing drawn; between them, as over the one before.
pub fn drop_slot(hits: &[(u16, u16, TabHit)], x: u16) -> Option<usize> {
    let (first, last) = (hits.first()?, hits.last()?);
    let x = x.clamp(first.0, last.1.saturating_sub(1).max(first.0));
    // A blank the bar left between its last tab and `›` counts as that tab's.
    let hit = hits.iter().rev().find(|h| x >= h.0)?.2;
    match hit {
        // `‹`: the hidden tab before the first one drawn; `›`: the one after the last.
        TabHit::More(i) if first.2 == hit => Some(i),
        TabHit::More(i) => Some(i + 1),
        TabHit::Tab(i) | TabHit::Close(i) => {
            let of_tab = |h: &&(u16, u16, TabHit)| matches!(h.2, TabHit::Tab(j) | TabHit::Close(j) if j == i);
            let start = hits.iter().filter(of_tab).map(|h| h.0).min()?;
            let end = hits.iter().filter(of_tab).map(|h| h.1).max()?;
            Some(if (x - start) * 2 < end - start { i } else { i + 1 })
        }
    }
}

/// The place a tab at `from` ends up at when dropped at `slot` (a place between tabs, counted
/// before the move).
pub fn slot_index(from: usize, slot: usize) -> usize {
    if slot > from { slot - 1 } else { slot }
}

impl App {
    /// A left press on tab `index` of the tab bar at column `x` made it active: a drag may
    /// follow.
    pub(super) fn arm_tab_drag(&mut self, index: usize, x: u16) {
        if self.tabs.active_index() != index {
            return;
        }
        self.tab_drag = Some(TabDrag { tab: self.tab().id, press_x: x, lifted: false, slot: None });
    }

    /// The tab being dragged, as the tab bar draws it: its place and where it would land
    /// (`None`: nowhere, the pointer is too far from the bar). Only once it is lifted.
    pub fn tab_drag_view(&self) -> Option<(usize, Option<usize>)> {
        let d = self.tab_drag.filter(|d| d.lifted && !self.modal_open())?;
        Some((self.tabs.position(d.tab)?, d.slot))
    }

    /// The left button moved to (x, y) while held: the pressed tab follows once it is lifted.
    /// `false`: no tab was pressed (the move is another drag's).
    pub(super) fn tab_drag_to(&mut self, x: u16, y: u16) -> bool {
        let Some(mut d) = self.tab_drag else { return false };
        if self.tabs.position(d.tab).is_none() {
            self.tab_drag = None;
            return true;
        }
        if !d.lifted && x.abs_diff(d.press_x) < LIFT_COLUMNS {
            return true;
        }
        if !d.lifted {
            // What the pointer lit before the press is not under it any more.
            self.pointer_on = None;
        }
        d.lifted = true;
        d.slot = self.drop_slot_at(x, y);
        self.tab_drag = Some(d);
        true
    }

    /// Where a tab released at (x, y) lands, by what the tab bar drew last.
    fn drop_slot_at(&self, x: u16, y: u16) -> Option<usize> {
        let bar = self.layout.tab_bar;
        if y.abs_diff(bar.y) > OUTSIDE_ROWS || bar.width == 0 {
            return None;
        }
        drop_slot(&self.tab_hits, x).map(|s| s.min(self.tabs.len()))
    }

    /// The left button was released at (x, y): a lifted tab lands there (nowhere: it stays).
    /// `false`: no tab was pressed.
    pub(super) fn tab_drag_release(&mut self, x: u16, y: u16) -> bool {
        let Some(d) = self.tab_drag.take() else { return false };
        if !d.lifted {
            return true;
        }
        let (Some(from), Some(slot)) = (self.tabs.position(d.tab), self.drop_slot_at(x, y)) else { return true };
        self.move_tab_to(from, slot_index(from, slot));
        true
    }

    /// `Esc`: a lifted tab goes back where it was. `false`: no tab was being dragged (a press
    /// that was not a drag yet is forgotten too).
    pub(super) fn cancel_tab_drag(&mut self) -> bool {
        self.tab_drag.take().is_some_and(|d| d.lifted)
    }

    /// `tab.move_left` / `tab.move_right`: the active tab one place over (not past an end).
    pub(super) fn move_active_tab(&mut self, right: bool) {
        if self.tabs.is_empty() {
            return;
        }
        let from = self.tabs.active_index();
        let to = if right { from + 1 } else { from.saturating_sub(1) };
        self.move_tab_to(from, to.min(self.tabs.len() - 1));
    }

    /// Move the tab at `from` to place `to`; the new order is written to the workspace state.
    pub(super) fn move_tab_to(&mut self, from: usize, to: usize) {
        if self.tabs.move_tab(from, to) {
            self.save_workspace();
        }
    }
}

#[cfg(test)]
mod tests;
