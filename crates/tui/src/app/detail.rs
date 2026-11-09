//! The detail pane under the results: its tabs and state.

/// The tabs of the result inspector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailTab {
    /// The selected cell's whole value, its type and length.
    Cell,
    /// Every column of the selected row, `name: value`: the tab the inspector starts on.
    #[default]
    Row,
}

impl DetailTab {
    pub fn other(self) -> Self {
        match self {
            DetailTab::Cell => DetailTab::Row,
            DetailTab::Row => DetailTab::Cell,
        }
    }
}

/// The result detail: shown or not (one key toggles it; `detail_view` decides
/// whether it is a panel or a status bar preview) and the panel's tab. It follows the grid's
/// selection.
#[derive(Clone, Copy, Debug)]
pub struct Detail {
    pub visible: bool,
    pub tab: DetailTab,
    /// The Row tab's first column on screen: it scrolls only to keep the grid's column in view.
    pub first: usize,
}

impl Default for Detail {
    fn default() -> Self {
        Self { visible: true, tab: DetailTab::Row, first: 0 }
    }
}
