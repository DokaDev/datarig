//! The cells a copy took from the result grid flash once the copy reached the clipboard
//! (`[ui] yank_flash`, Neovim's highlight on yank). The flash is bound to the tab, the result
//! and the session generation it was copied from: another result or tab never shows it.
//!
//! On a theme whose flash colours are RGB the cells light up at the full flash colour, hold it
//! for [`HOLD`], then fade back to how each cell is drawn anyway over [`FADE`], one frame every
//! [`FRAME`] (a timer per frame while it fades, none before or after). A flash colour the
//! terminal picks (its own 16 colours, 256 colours, its default) cannot be mixed: the flash is
//! then on for [`YANK_FLASH`] and off at once, one timer deadline.

use super::*;
use datarig_core::config::YankFlash;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::ops::Range;

/// How long copied cells flash when the flash cannot fade (on, then off). Neovim flashes a yank
/// for 150 ms; a terminal frame can reach the screen late (over SSH, inside tmux), and at 150 ms
/// a flash of one cell is easily missed, so it stays twice as long, still well before the next
/// key of a quick hand.
pub const YANK_FLASH: Duration = Duration::from_millis(300);

/// How long a fading flash stays at its full colour before it fades: long enough for the frame
/// that shows it to reach the screen and be seen as a flash, not as a fade from the start.
pub const HOLD: Duration = Duration::from_millis(80);

/// How long a flash takes to fade back. Fast at first and slowing down at the end (ease-out),
/// so by [`YANK_FLASH`] it is nearly gone (about a tenth of its colour left), as the flash that
/// cannot fade; the tail only settles it into the cell's own colour.
pub const FADE: Duration = Duration::from_millis(320);

/// One frame of a fade every this long: eight frames for [`FADE`]. 25 frames a second with the
/// colour moving a few steps a frame reads as a smooth fade; more frames would only cost
/// wakeups, fewer would show the steps.
pub const FRAME: Duration = Duration::from_millis(40);

/// Cells of a result a copy took (absolute row and column indexes), flashing since `since`.
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
    pub since: Instant,
}

impl CopyFlash {
    /// Whether cell (row, col) flashes.
    pub fn covers(&self, row: usize, col: usize) -> bool {
        self.rows.contains(&row) && self.cols.binary_search(&col).is_ok()
    }

    /// When the flash is over: [`HOLD`] + [`FADE`] after the copy when it fades, else
    /// [`YANK_FLASH`].
    pub fn end(&self, fades: bool) -> Instant {
        self.since + if fades { HOLD + FADE } else { YANK_FLASH }
    }

    /// How much of the flash colour a fading flash shows at `now`: 1 while it holds, then down
    /// to 0 at its end, eased out.
    pub fn level(&self, now: Instant) -> f32 {
        let fading = now.saturating_duration_since(self.since).saturating_sub(HOLD);
        let left = 1.0 - (fading.as_secs_f32() / FADE.as_secs_f32()).min(1.0);
        left * left
    }

    /// When the next frame of the flash is due at `now`: its end when it cannot fade; while it
    /// fades the next whole [`FRAME`] after its hold (never one during the hold, nothing changes
    /// there), the last one at its end. (A cell that cannot be mixed in a fading flash goes off
    /// with the first frame from [`YANK_FLASH`] on, no wakeup of its own.)
    pub fn due(&self, now: Instant, fades: bool) -> Instant {
        if !fades {
            return self.end(false);
        }
        let hold = self.since + HOLD;
        let frames = (FADE.as_nanos() / FRAME.as_nanos()) as u32;
        let done = now.saturating_duration_since(hold).as_nanos() / FRAME.as_nanos();
        hold + FRAME * u32::try_from(done + 1).unwrap_or(u32::MAX).min(frames)
    }
}

/// Whether a flash can fade on theme `th`: its flash background (and its text colour, if it sets
/// one) are RGB colours, and so are the backgrounds a grid cell is drawn on (surfaces,
/// selection, range, cursor line, hover) and the text. A grid in the terminal's colours would
/// only redraw the same full flash frame after frame.
pub fn fades(th: &crate::theme::Theme) -> bool {
    let rgb = |c: Option<Color>| matches!(c, None | Some(Color::Rgb(..)));
    let style = th.yank_flash;
    matches!(style.bg, Some(Color::Rgb(..)))
        && rgb(style.fg)
        && [th.bg, th.surface, th.surface_alt, th.fg].into_iter().all(|c| rgb(Some(c)))
        && [th.selection, th.selection_unfocused(), th.range, th.cursor_line, th.hover].iter().all(|s| rgb(s.bg))
}

/// `a` mixed into `b`: `level` 1 = `a`, 0 = `b`. `None` unless both are RGB.
fn mix(a: Color, b: Color, level: f32) -> Option<Color> {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else { return None };
    let ch = |a: u8, b: u8| (f32::from(b) + (f32::from(a) - f32::from(b)) * level).round().clamp(0.0, 255.0) as u8;
    Some(Color::Rgb(ch(ar, br), ch(ag, bg), ch(ab, bb)))
}

/// Draws flash `style` over the cells of `area` already drawn, at `level` (see
/// [`CopyFlash::level`]) of the way from each cell's own colours to the flash's, or at full when
/// `level` is `None` (a flash that cannot fade, still on). A cell whose own colours are not RGB
/// cannot be mixed either: it shows the full flash while `on`, as before. The flash covers the
/// cursor's cell too, whose reversed text would hide a reversed flash: reversed colours are
/// read as drawn.
pub fn paint(buf: &mut Buffer, area: Rect, style: Style, level: Option<f32>, on: bool) {
    let full = Style::new().remove_modifier(Modifier::REVERSED | Modifier::DIM).patch(style);
    let Some(level) = level else {
        if on {
            buf.set_style(area, full);
        }
        return;
    };
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let (fg, bg) = if reversed { (cell.bg, cell.fg) } else { (cell.fg, cell.bg) };
            let flash_bg = style.bg.unwrap_or(bg);
            let mixed = mix(flash_bg, bg, level).zip(match style.fg {
                Some(f) => mix(f, fg, level),
                None => Some(fg),
            });
            match mixed {
                Some((new_bg, new_fg)) => {
                    cell.modifier.remove(Modifier::REVERSED);
                    // Modifiers cannot be mixed: the flash's (and no DIM) for the first half
                    // of the fade, the cell's own after.
                    if level >= 0.5 {
                        cell.modifier.remove(Modifier::DIM | style.sub_modifier);
                        cell.modifier.insert(style.add_modifier);
                    }
                    cell.bg = new_bg;
                    cell.fg = new_fg;
                }
                None if on => {
                    cell.set_style(full);
                }
                None => {}
            }
        }
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
        self.copy_flash = Some(CopyFlash { tab, result, generation, rows, cols, since: self.now() });
    }

    /// Whether the flash fades on the current theme (see [`fades`]).
    pub fn copy_flash_fades(&self) -> bool {
        fades(&self.theme)
    }

    /// The flash to draw over the active tab's grid now: none once it ended, or when the tab,
    /// its result or its session is not the one copied from.
    pub fn copy_flash_shown(&self) -> Option<&CopyFlash> {
        let f = self.copy_flash.as_ref()?;
        let t = self.tab();
        let same = matches!(&t.results, Results::Rows(rs) if rs.id == f.result);
        let on = self.now() < f.end(self.copy_flash_fades());
        (same && t.id == f.tab && t.exec.generation == f.generation && on).then_some(f)
    }

    /// The flash's next frame (its end, when it cannot fade), the timer deadline it needs.
    pub(super) fn copy_flash_due(&self, now: Instant) -> Option<Instant> {
        self.copy_flash.as_ref().map(|f| f.due(now, self.copy_flash_fades()))
    }

    /// The flash ended by `now`: it goes (the frame after this tick draws the cells as before).
    pub(super) fn copy_flash_tick(&mut self, now: Instant) {
        let fades = self.copy_flash_fades();
        if self.copy_flash.as_ref().is_some_and(|f| now >= f.end(fades)) {
            self.copy_flash = None;
        }
    }
}
