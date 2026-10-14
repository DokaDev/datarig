//! Moving tabs in the tab bar: by the mouse (press a tab, drag it sideways, release where it
//! goes) and by keys (`tab.move_left` / `tab.move_right`, `:tabmove`).
//!
//! A press on a tab makes it active, as a click does, and arms a drag. Only once the pointer
//! went [`LIFT_COLUMNS`] columns sideways is the tab lifted: drawn in the theme's selection
//! style where the pointer holds it, over the tabs next to it, as a browser does. The other
//! tabs keep a gap of its width where it would land, so the bar shows the order a release
//! keeps (the numbers are places, so they follow at once). Once the lifted tab went past the
//! middle of a tab next to the gap, that tab slides over into the gap ([`SLIDE`], a few frames,
//! then no timer at all); a pointer that jumped over several tabs at once (a remote terminal
//! reports moves coarsely) passes each of them in turn. Over a scroll mark (`‹` / `›`) each move
//! goes past the next hidden tab on that side, so the bar scrolls by one tab at a time. Before
//! the lift a release is the click it was.
//!
//! More than [`OUTSIDE_ROWS`] rows above or below the bar the tab has nowhere to land: it goes
//! back to its own place and a release there drops nothing; back near the bar, the drag goes
//! on. `Esc`, a dialog coming on top, a press of another button, a lost release or another set
//! or order of tabs end the drag: every tab slides back to where it was.
//!
//! A console's own name (`console 3`) does not change. The new order is written to the
//! workspace state at once.

use super::*;

/// How far sideways (in columns) the pointer goes from the press before the tab is lifted.
pub const LIFT_COLUMNS: u16 = 2;

/// How many rows above or below the tab bar a lifted tab still has a place to land.
pub const OUTSIDE_ROWS: u16 = 2;

/// How long a tab takes to slide into its new place. Long enough to be seen as a move (a tab
/// goes 10–30 columns), short enough never to be in the way of the next one: about what a
/// browser takes.
pub const SLIDE: Duration = Duration::from_millis(100);

/// One frame of a slide every this long while one is under way (five frames for [`SLIDE`]); no
/// timer otherwise.
pub const SLIDE_FRAME: Duration = Duration::from_millis(20);

/// A press on a tab of the tab bar, and the drag that may follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabDrag {
    /// The pressed tab (made active by the press).
    pub tab: TabId,
    /// The column the press was at.
    pub press_x: u16,
    /// The pointer went far enough sideways: the tab is being dragged.
    pub lifted: bool,
    /// The tabs, in order, when the tab was lifted: other tabs or another order since end the
    /// drag.
    pub order: Vec<TabId>,
    /// How many columns right of the tab's left end the pointer holds it.
    pub grab: u16,
    /// The pointer's column and row.
    pub x: u16,
    pub y: u16,
    /// The tab's place among the tabs as the bar shows it: where a release puts it.
    pub slot: usize,
    /// The pointer is too far from the bar: the tab is back at its own place and lands nowhere.
    pub far: bool,
}

impl TabDrag {
    /// The tab's place when it was lifted.
    pub fn from(&self) -> usize {
        self.order.iter().position(|t| *t == self.tab).unwrap_or(0)
    }
}

/// The order the bar shows `n` tabs in while tab `from` is dragged to place `slot`: the
/// others in their order, the dragged one at `slot`.
pub fn shown_order(n: usize, from: usize, slot: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).filter(|i| *i != from).collect();
    if from < n {
        order.insert(slot.min(order.len()), from);
    }
    order
}

/// The place a dragged tab takes among the others, as the bar shows them: `others` are their
/// widths in order, the dragged tab (`width` wide) is at place `slot`, the bar shows places
/// `first..=last` from column `left`, and the dragged tab is drawn from column `x`. Each time
/// it went past the middle of the tab next to its place (and that tab is shown), that tab moves
/// over to the other side; as many times as it went past (the pointer may have jumped).
pub fn follow(others: &[u16], slot: usize, first: usize, last: usize, left: u16, x: u16) -> usize {
    let mut slot = slot.min(others.len());
    if slot < first || slot > last {
        return slot;
    }
    let x = 2 * i32::from(x);
    // Where the gap the dragged tab leaves starts.
    let mut gap = i32::from(left) + others[first..slot].iter().map(|w| i32::from(*w)).sum::<i32>();
    while slot < others.len() && slot < last && x > 2 * gap + i32::from(others[slot]) {
        gap += i32::from(others[slot]);
        slot += 1;
    }
    while slot > first && x < 2 * gap - i32::from(others[slot - 1]) {
        gap -= i32::from(others[slot - 1]);
        slot -= 1;
    }
    slot
}

/// The place a tab at `from` ends up at when dropped at `slot` (a place between tabs, counted
/// before the move).
pub fn slot_index(from: usize, slot: usize) -> usize {
    if slot > from { slot - 1 } else { slot }
}

/// Tabs sliding to new places in the tab bar after a drag moved them (or put them back). The
/// bar says where each shown tab belongs every frame ([`TabSlide::place`]); a tab whose place
/// changed because of the drag slides there from where it was drawn, any other change of
/// places (a tab opened, closed or switched to, the screen resized) is drawn at once.
#[derive(Clone, Debug, Default)]
pub struct TabSlide {
    /// The tabs drawn last: where each belonged.
    last: Vec<(TabId, i32)>,
    /// Slides under way.
    moving: Vec<Slide>,
    /// The next change of places is the drag's: the tabs it moves slide.
    pub(crate) armed: bool,
    /// The tab dropped or put back last: drawn over the others while it slides.
    pub(crate) top: Option<TabId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slide {
    tab: TabId,
    from: i32,
    to: i32,
    since: Instant,
}

impl Slide {
    /// The column it is drawn at, at `now`: fast at first, slowing down into its place.
    fn at(&self, now: Instant) -> i32 {
        let done = now.saturating_duration_since(self.since).as_secs_f64() / SLIDE.as_secs_f64();
        if done >= 1.0 {
            return self.to;
        }
        let left = (1.0 - done) * (1.0 - done);
        self.to + (f64::from(self.from - self.to) * left).round() as i32
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) >= SLIDE
    }
}

impl TabSlide {
    /// The columns to draw the shown tabs at, at `now`, given where each belongs (`places`:
    /// tab, column, and whether it is held under the pointer, which never slides).
    pub fn place(&mut self, places: &[(TabId, i32, bool)], now: Instant) -> Vec<i32> {
        let armed = std::mem::take(&mut self.armed);
        let mut out = Vec::with_capacity(places.len());
        for &(tab, to, held) in places {
            let was = self.last.iter().find(|l| l.0 == tab).map(|l| l.1);
            let moving = self.moving.iter().position(|m| m.tab == tab);
            if held || was.is_some_and(|w| w != to) {
                // From where it is drawn now, to its new place; or there at once.
                let from = moving.map(|m| self.moving[m].at(now)).or(was).unwrap_or(to);
                if let Some(m) = moving {
                    self.moving.swap_remove(m);
                }
                if armed && !held && from != to {
                    self.moving.push(Slide { tab, from, to, since: now });
                }
            }
            out.push(self.moving.iter().find(|m| m.tab == tab).map_or(to, |m| m.at(now)));
        }
        // A tab not shown any more stops sliding.
        self.moving.retain(|m| places.iter().any(|p| p.0 == m.tab));
        self.last = places.iter().map(|p| (p.0, p.1)).collect();
        if self.top.is_some_and(|t| !self.moving(t)) {
            self.top = None;
        }
        out
    }

    /// Whether tab `tab` slides now.
    pub fn moving(&self, tab: TabId) -> bool {
        self.moving.iter().any(|m| m.tab == tab)
    }

    /// When the next frame of a slide is due (`None`: nothing slides).
    pub fn due(&self, now: Instant) -> Option<Instant> {
        let end = self.moving.iter().map(|m| m.since + SLIDE).max()?;
        Some(end.min(now + SLIDE_FRAME))
    }

    /// Slides over by `now` end (the frame after this tick draws their tabs in place).
    pub fn tick(&mut self, now: Instant) {
        self.moving.retain(|m| !m.done(now));
        if self.top.is_some_and(|t| !self.moving(t)) {
            self.top = None;
        }
    }
}

impl App {
    /// A left press on tab `index` of the tab bar at column `x` made it active: a drag may
    /// follow.
    pub(super) fn arm_tab_drag(&mut self, index: usize, x: u16) {
        if self.tabs.active_index() != index {
            return;
        }
        let tab = self.tab().id;
        self.tab_drag = Some(TabDrag {
            tab,
            press_x: x,
            lifted: false,
            order: Vec::new(),
            grab: 0,
            x,
            y: 0,
            slot: index,
            far: false,
        });
    }

    /// The tab being dragged, while it is lifted (and no dialog is on top, and the tabs are
    /// the ones it was lifted among).
    pub(crate) fn tab_drag_live(&self) -> Option<&TabDrag> {
        let d = self.tab_drag.as_ref().filter(|d| d.lifted && !self.modal_open())?;
        self.tabs.iter().map(|t| t.id).eq(d.order.iter().copied()).then_some(d)
    }

    /// The tab being dragged, as the tab bar shows it: its place, and the place it is shown
    /// at, where a release puts it (`None`: nowhere, the pointer is too far from the bar). Only
    /// once it is lifted.
    pub fn tab_drag_view(&self) -> Option<(usize, Option<usize>)> {
        let d = self.tab_drag_live()?;
        Some((d.from(), (!d.far).then_some(d.slot)))
    }

    /// The left button moved to (x, y) while held: the pressed tab follows once it is lifted.
    /// `false`: no tab was pressed (the move is another drag's).
    pub(super) fn tab_drag_to(&mut self, x: u16, y: u16) -> bool {
        let Some(mut d) = self.tab_drag.take() else { return false };
        if self.tabs.position(d.tab).is_none() || (d.lifted && self.tab_drag_live_of(&d).is_none()) {
            self.end_lifted(&d);
            return true;
        }
        if !d.lifted && x.abs_diff(d.press_x) < LIFT_COLUMNS {
            self.tab_drag = Some(d);
            return true;
        }
        if !d.lifted {
            // What the pointer lit before the press is not under it any more.
            self.pointer_on = None;
            let from = self.tabs.position(d.tab).unwrap_or(0);
            d.order = self.tabs.iter().map(|t| t.id).collect();
            d.slot = from;
            d.lifted = true;
            // Held where the press was, on the tab as it was drawn (sliding, or scrolled by the
            // press: within the tab).
            let at = self.tab_geom.drawn.get(from).copied().flatten();
            let width = i32::from(self.tab_geom.widths.get(from).copied().unwrap_or(1).max(1));
            d.grab = at.map_or(0, |a| (i32::from(d.press_x) - a).clamp(0, width - 1) as u16);
        }
        self.tab_drag_follow(&mut d, x, y, true);
        self.tab_drag = Some(d);
        true
    }

    /// The order check of [`App::tab_drag_live`] for a drag taken out of the app.
    fn tab_drag_live_of<'a>(&self, d: &'a TabDrag) -> Option<&'a TabDrag> {
        self.tabs.iter().map(|t| t.id).eq(d.order.iter().copied()).then_some(d)
    }

    /// The lifted tab `d` follows the pointer at (x, y): its place, by what the tab bar drew
    /// last (`scroll`: over a scroll mark, past the next hidden tab). A change of place makes
    /// the tabs it moves slide.
    fn tab_drag_follow(&mut self, d: &mut TabDrag, x: u16, y: u16, scroll: bool) {
        let bar = self.layout.tab_bar;
        let far = bar.width == 0 || y.abs_diff(bar.y) > OUTSIDE_ROWS;
        let from = d.from();
        let slot = if far { from } else { self.tab_drag_slot(d, x, scroll).unwrap_or(d.slot) };
        if slot != d.slot || far != d.far {
            self.tab_slide.armed = true;
        }
        d.slot = slot;
        d.far = far;
        (d.x, d.y) = (x, y);
    }

    /// Where the lifted tab `d` goes with the pointer at column `x`, by what the tab bar drew
    /// last (`None`: it drew another set of tabs).
    fn tab_drag_slot(&self, d: &TabDrag, x: u16, scroll: bool) -> Option<usize> {
        let g = &self.tab_geom;
        let n = d.order.len();
        let from = d.from();
        if g.widths.len() != n || g.order != shown_order(n, from, d.slot) || n == 0 {
            return None;
        }
        let others: Vec<u16> = (0..n).filter(|i| *i != from).map(|i| g.widths[i]).collect();
        let width = g.widths[from];
        let (first, last) = (g.first.min(n - 1), g.last.min(n - 1));
        let shown: u16 = g.order[first..=last].iter().map(|i| g.widths[*i]).sum();
        let end = g.stop.min(g.left + shown);
        let left = (i32::from(x) - i32::from(d.grab))
            .clamp(i32::from(g.left), i32::from(end.saturating_sub(width).max(g.left)));
        let mut slot = follow(&others, d.slot, first, last, g.left, left as u16);
        // Over a scroll mark (or past it), next to the last tab shown on that side: past the
        // next hidden tab, one per move.
        if scroll && x >= g.stop && last + 1 < n && slot == last {
            slot += 1;
        } else if scroll && x < g.left && first > 0 && slot == first {
            slot -= 1;
        }
        Some(slot)
    }

    /// The left button was released at (x, y): a lifted tab lands where it is shown (nowhere:
    /// it stays). `false`: no tab was pressed.
    pub(super) fn tab_drag_release(&mut self, x: u16, y: u16) -> bool {
        let Some(mut d) = self.tab_drag.take() else { return false };
        if !d.lifted {
            return true;
        }
        if self.tab_drag_live_of(&d).is_none() {
            self.end_lifted(&d);
            return true;
        }
        // Where it was shown, when released where the last move was; elsewhere where the
        // pointer takes it, but never past a hidden tab no frame showed it next to.
        if (x, y) != (d.x, d.y) {
            self.tab_drag_follow(&mut d, x, y, false);
        }
        self.end_lifted(&d);
        if !d.far {
            self.move_tab_to(d.from(), d.slot);
        }
        true
    }

    /// `Esc`: a lifted tab goes back where it was. `false`: no tab was being dragged (a press
    /// that was not a drag yet is forgotten too).
    pub(super) fn cancel_tab_drag(&mut self) -> bool {
        self.end_tab_drag()
    }

    /// The drag ends where it is (a dialog, another button, a lost release): every tab goes back
    /// to its place. `true`: a tab was lifted.
    pub(super) fn end_tab_drag(&mut self) -> bool {
        let Some(d) = self.tab_drag.take() else { return false };
        self.end_lifted(&d);
        d.lifted
    }

    /// A lifted tab is let go: the tabs slide to where they belong now, the tab over the
    /// others.
    fn end_lifted(&mut self, d: &TabDrag) {
        // On a screen too small for the bar nothing is drawn that could slide.
        if d.lifted && !self.layout.too_small {
            self.tab_slide.armed = true;
            self.tab_slide.top = Some(d.tab);
        }
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
