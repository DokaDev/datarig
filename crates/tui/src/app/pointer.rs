//! Pointer state: clicks, drags, and where tabs and result tabs were drawn last (hit tests).

use super::*;

#[derive(Default)]
pub(crate) struct PointerState {
    pub(super) last_click: Option<(Instant, u16, u16)>,
    /// Clicks in a row at the same place (1, 2 for a double click, 3 for a triple click).
    pub(super) clicks: u8,
    /// A left-button drag in progress.
    pub(super) drag: Option<Drag>,
    /// Where the pointer was at the last move of a grid drag (the wheel extends from there).
    pub(super) drag_at: Option<(u16, u16)>,
    /// Where each result tab of the strip was drawn last: its columns and the statement it
    /// shows (`None`: Messages).
    pub(crate) strip_hits: Vec<(u16, u16, Option<usize>, tabs::ResultView)>,
    /// A press on a tab of the tab bar, and the drag of that tab that may follow.
    pub(crate) tab_drag: Option<tab_drag::TabDrag>,
    /// Where each tab and scroll mark of the document tab bar was drawn last.
    pub(crate) tab_hits: Vec<(u16, u16, crate::widgets::tabbar::TabHit)>,
    /// How the document tab bar laid its tabs out last (a dragged tab follows the pointer by it).
    pub(crate) tab_geom: crate::widgets::tabbar::BarGeom,
    /// Tabs sliding to their places after a drag moved them.
    pub(crate) tab_slide: tab_drag::TabSlide,
    /// The tab the pointer was over its grid or inspector in (a grid cell or an inspector entry
    /// lit is of that tab's result only).
    pub(crate) pointer_tab: Option<TabId>,
    /// The column of the inspector's Row tab a press armed, with the result it is of (it is
    /// picked on the release, over the same column of the same result).
    pub(crate) detail_press: overlay::Press<(u64, usize)>,
    /// The column the last release over the inspector's Row tab picked (a second click on it
    /// shows its value whole); none once a press there armed nothing.
    pub(super) detail_picked: Option<(u64, usize)>,
    /// The kind of the top dialog the last frame saw (`App::note_top`).
    pub(super) last_top: Option<OverlayKind>,
}
