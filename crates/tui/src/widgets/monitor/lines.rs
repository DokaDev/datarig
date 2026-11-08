//! Lines of recent values in braille dots, several series on one shared scale: what the
//! Overview's charts draw. Two values per cell (a cell is 2 dots wide and 4 high), the newest at
//! the right edge, the scale from zero to the largest value shown; a value that is not there
//! (`None`) breaks its line, never drawn as zero.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// The braille bit of dot (`dx`, `dy`) of a cell (2 wide, 4 high).
fn dot_bit(dx: usize, dy: usize) -> u8 {
    const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    BITS[dx][dy]
}

/// The largest value the newest `2 * w` of every series show, 0 when none is above it.
pub(crate) fn top(series: &[&[Option<f64>]], w: u16) -> f64 {
    let n = usize::from(w) * 2;
    series
        .iter()
        .flat_map(|s| s[s.len().saturating_sub(n)..].iter().flatten())
        .filter(|v| v.is_finite())
        .fold(0.0_f64, |m, v| m.max(*v))
}

/// The dots of `series` in a `w`×`h` plot scaled from 0 to `max`, per cell: its dots and the
/// series (from 1) drawn last in it (0: none).
pub(crate) fn raster(series: &[&[Option<f64>]], max: f64, w: u16, h: u16) -> Vec<(u8, u8)> {
    let (dw, dh) = (usize::from(w) * 2, usize::from(h) * 4);
    let mut cells = vec![(0u8, 0u8); usize::from(w) * usize::from(h)];
    if dw == 0 || dh == 0 {
        return cells;
    }
    let mut set = |x: usize, y: usize, s: usize| {
        let cell = &mut cells[(y / 4) * usize::from(w) + x / 2];
        cell.0 |= dot_bit(x % 2, y % 4);
        cell.1 = s as u8 + 1;
    };
    for (s, values) in series.iter().enumerate() {
        let shown = &values[values.len().saturating_sub(dw)..];
        // The newest value in the rightmost dot column.
        let start = dw - shown.len();
        let mut prev: Option<usize> = None;
        for (i, v) in shown.iter().enumerate() {
            let Some(v) = v.filter(|v| v.is_finite()) else {
                prev = None;
                continue;
            };
            let f = if max > 0.0 { (v.max(0.0) / max).min(1.0) } else { 0.0 };
            let y = ((1.0 - f) * (dh - 1) as f64).round() as usize;
            let x = start + i;
            // Joined to the value before by the dots between them in this column.
            let (lo, hi) = match prev {
                Some(p) if p < y => (p + 1, y),
                Some(p) if p > y => (y, p - 1),
                _ => (y, y),
            };
            for yy in lo..=hi {
                set(x, yy, s);
            }
            set(x, y, s);
            prev = Some(y);
        }
    }
    cells
}

/// `series` (each with its colour) drawn in `area` on one scale from 0 to `max`.
pub(crate) fn draw(series: &[(&[Option<f64>], Color)], max: f64, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let values: Vec<&[Option<f64>]> = series.iter().map(|(v, _)| *v).collect();
    let cells = raster(&values, max, area.width, area.height);
    for (i, &(bits, s)) in cells.iter().enumerate() {
        if bits == 0 {
            continue;
        }
        let w = usize::from(area.width);
        let (x, y) = (area.x + (i % w) as u16, area.y + (i / w) as u16);
        let ch = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
        let color = series[usize::from(s) - 1].1;
        buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(cells: &[(u8, u8)], w: usize) -> Vec<String> {
        cells
            .chunks(w)
            .map(|row| {
                row.iter()
                    .map(|(b, _)| if *b == 0 { ' ' } else { char::from_u32(0x2800 + u32::from(*b)).unwrap() })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn two_values_a_cell_newest_right_on_a_shared_scale() {
        let a = [Some(0.0), Some(4.0), Some(8.0), Some(8.0)];
        let b = [Some(0.0), Some(0.0)];
        let max = top(&[&a, &b], 3);
        assert_eq!(max, 8.0);
        let cells = raster(&[&a, &b], max, 3, 1);
        // Four values fill the two right cells; the first cell is empty.
        assert_eq!(text(&cells, 3), [" ⡠⣋"]);
        assert_eq!(cells[1], (0x40 | 0x20, 1));
        // The second series' two zeros sit in the bottom row of the rightmost cell, drawn last.
        assert_eq!(cells[2], (0x01 | 0x02 | 0x08 | 0x40 | 0x80, 2));
    }

    #[test]
    fn a_missing_value_breaks_the_line_and_is_never_zero() {
        let a = [Some(8.0), None, Some(8.0), Some(0.0)];
        let cells = raster(&[&a], 8.0, 2, 1);
        // The top dot alone, nothing for the gap, then a line down to zero.
        assert_eq!(cells[0].0, 0x01);
        assert_eq!(cells[1].0, 0x01 | 0x10 | 0x20 | 0x80);
        let gaps = [None, None];
        assert!(raster(&[&gaps], 1.0, 1, 1).iter().all(|c| c.0 == 0));
    }

    #[test]
    fn all_zero_lies_on_the_bottom_and_only_the_newest_that_fit_scale() {
        let zeros = [Some(0.0); 4];
        assert_eq!(top(&[&zeros], 2), 0.0);
        assert_eq!(text(&raster(&[&zeros], 0.0, 2, 1), 2), ["⣀⣀"]);
        // An old peak no longer shown does not scale what is.
        let a = [Some(100.0), Some(1.0), Some(2.0)];
        assert_eq!(top(&[&a], 1), 2.0);
    }
}
