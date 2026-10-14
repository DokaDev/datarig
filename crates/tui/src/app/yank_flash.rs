//! The cells a copy took from the result grid flash for a moment once the copy reached the
//! clipboard (`[ui] yank_flash`, Neovim's highlight on yank). The flash is bound to the tab, the
//! result and the session generation it was copied from: another result or tab never shows it.
//! Its end is one timer deadline ([`App::next_tick`]), not an animation.

use super::*;
use datarig_core::config::YankFlash;
use std::ops::Range;

/// How long copied cells flash. Neovim flashes a yank for 150 ms; a terminal frame can reach the
/// screen late (over SSH, inside tmux), and at 150 ms a flash of one cell is easily missed, so
/// it stays twice as long, still well before the next key of a quick hand.
pub const YANK_FLASH: Duration = Duration::from_millis(300);

/// Cells of a result a copy took (absolute row and column indexes), shown until `until`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyFlash {
    pub tab: TabId,
    /// [`ResultSet::id`] of the result copied from.
    pub result: u64,
    /// The tab's session generation when it was copied.
    pub generation: u64,
    pub rows: Range<usize>,
    /// Sorted.
    pub cols: Vec<usize>,
    pub until: Instant,
}

impl CopyFlash {
    /// Whether cell (row, col) flashes.
    pub fn covers(&self, row: usize, col: usize) -> bool {
        self.rows.contains(&row) && self.cols.binary_search(&col).is_ok()
    }
}

impl App {
    /// A copy of cells `rows` × `cols` of the active tab's result reached the clipboard: they
    /// flash (unless `[ui] yank_flash = "off"`, or the tab shows its chart, plan or messages
    /// instead of the rows), a flash still on screen starting over.
    pub(super) fn flash_copied(&mut self, rows: Range<usize>, mut cols: Vec<usize>) {
        self.copy_flash = None;
        if self.prefs.yank_flash == YankFlash::Off {
            return;
        }
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return };
        if t.exec.view != tabs::ResultView::Rows {
            return;
        }
        cols.sort_unstable();
        cols.dedup();
        let (tab, result, generation) = (t.id, rs.id, t.exec.generation);
        self.copy_flash = Some(CopyFlash { tab, result, generation, rows, cols, until: self.now() + YANK_FLASH });
    }

    /// The flash to draw over the active tab's grid now: none once it ended, or when the tab,
    /// its result or its session is not the one copied from.
    pub fn copy_flash_shown(&self) -> Option<&CopyFlash> {
        let f = self.copy_flash.as_ref()?;
        let t = self.tab();
        let same = matches!(&t.results, Results::Rows(rs) if rs.id == f.result);
        (same && t.id == f.tab && t.exec.generation == f.generation && self.now() < f.until).then_some(f)
    }

    /// The flash's end, the timer deadline it needs.
    pub(super) fn copy_flash_due(&self) -> Option<Instant> {
        self.copy_flash.as_ref().map(|f| f.until)
    }

    /// The flash ended by `now`: it goes (the frame after this tick draws the cells as before).
    pub(super) fn copy_flash_tick(&mut self, now: Instant) {
        if self.copy_flash.as_ref().is_some_and(|f| now >= f.until) {
            self.copy_flash = None;
        }
    }
}
