//! A one-row chart of recent values in eighth blocks, newest on the right: what the Monitor
//! tab's charts draw. The bars of the result charts use the same blocks.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Bars growing up, by eighths of a cell.
pub(crate) const UP: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The blocks for `values` (oldest first; `None` is a gap, drawn as nothing), at most `width` of
/// them (the newest), scaled from zero to the largest value shown. A value that is there is
/// never drawn as nothing: zero is the lowest block.
pub(crate) fn blocks(values: &[Option<f64>], width: usize) -> String {
    let shown = &values[values.len().saturating_sub(width)..];
    let max = shown.iter().flatten().copied().fold(0.0_f64, f64::max);
    shown
        .iter()
        .map(|v| match v {
            None => UP[0],
            Some(v) if max <= 0.0 || !v.is_finite() => UP[1],
            Some(v) => UP[1 + ((v.max(0.0) / max) * 7.0).round().clamp(0.0, 7.0) as usize],
        })
        .collect()
}

/// [`blocks`] drawn in `area`'s first row, right-aligned.
pub(crate) fn draw(values: &[Option<f64>], area: Rect, style: Style, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = blocks(values, area.width as usize);
    let n = text.chars().count() as u16;
    buf.set_stringn(area.x + area.width - n, area.y, &text, n as usize, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_to_the_largest_value_shown_and_keeps_gaps() {
        assert_eq!(blocks(&[Some(0.0), Some(4.0), None, Some(8.0)], 10), "▁▅ █");
        // Only the newest that fit: the old peak no longer scales them.
        assert_eq!(blocks(&[Some(100.0), Some(1.0), Some(2.0)], 2), "▅█");
        // All zero: the lowest block, never nothing.
        assert_eq!(blocks(&[Some(0.0), Some(0.0)], 4), "▁▁");
        assert_eq!(blocks(&[], 4), "");
    }
}
