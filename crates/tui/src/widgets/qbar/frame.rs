//! The frame of the query bar's popups: a box placed by the part it opened on ([`anchored`]),
//! drawn opaque with rounded corners, its title on the top border and its hint line on the
//! bottom border ([`frame`]).

use crate::text::width;
use crate::theme;
use crate::widgets::condition_pane::fill;
use crate::widgets::popup::clear_overlay;
use crate::widgets::put;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Widget};

/// A box of `w` by `h` cells for a popup opened on `anchor`, inside `bounds`: under the anchor,
/// its left edge on the anchor's; above it only where there is no room below and there is
/// above; moved left where it would pass the right edge; as tall as the side it is on allows
/// (never taller than `bounds`).
pub(crate) fn anchored(anchor: Rect, w: u16, h: u16, bounds: Rect) -> Rect {
    let w = w.min(bounds.width);
    let below = bounds.bottom().saturating_sub(anchor.bottom().max(bounds.y));
    let above = anchor.y.min(bounds.bottom()).saturating_sub(bounds.y);
    let (y, h) = if h <= below || below >= above {
        (anchor.bottom().max(bounds.y), h.min(below))
    } else {
        let h = h.min(above);
        (anchor.y - h, h)
    };
    let x = anchor.x.min(bounds.right().saturating_sub(w)).max(bounds.x);
    Rect::new(x, y.min(bounds.bottom()), w, h)
}

/// Draw the frame of a popup over `rect`: what is under it cleared, the theme's background,
/// a rounded border, `title` (parts and their styles) on the top border and `hint` on the
/// bottom border, both from the third column and cut where the border ends. Returns the cells
/// inside the border.
pub(crate) fn frame(buf: &mut Buffer, rect: Rect, title: &[(String, Style)], hint: &[(String, Style)]) -> Rect {
    let rect = rect.intersection(buf.area);
    if rect.width < 4 || rect.height < 2 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    let th = theme::cur();
    clear_overlay(rect, buf);
    fill(buf, rect, th.base());
    Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(th.accent)).render(rect, buf);
    let end = rect.right().saturating_sub(2);
    for (parts, y) in [(title, rect.y), (hint, rect.bottom() - 1)] {
        let mut x = rect.x + 2;
        for (text, style) in parts {
            if x >= end {
                break;
            }
            x += put(buf, x, y, text, usize::from(end - x).min(width(text)), *style);
        }
    }
    Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect { x: 10, y: 5, width: 60, height: 20 };

    #[test]
    fn under_the_anchor_on_its_left_edge() {
        let at = Rect::new(20, 5, 8, 1);
        assert_eq!(anchored(at, 40, 10, BOUNDS), Rect::new(20, 6, 40, 10));
    }

    #[test]
    fn moved_left_to_stay_inside() {
        let at = Rect::new(60, 5, 8, 1);
        assert_eq!(anchored(at, 40, 10, BOUNDS), Rect::new(30, 6, 40, 10));
        // Wider than the bounds: as wide as they are.
        assert_eq!(anchored(at, 90, 4, BOUNDS), Rect::new(10, 6, 60, 4));
    }

    #[test]
    fn above_only_without_room_below() {
        let at = Rect::new(20, 20, 8, 1);
        // Four rows below fit a box of four.
        assert_eq!(anchored(at, 40, 4, BOUNDS), Rect::new(20, 21, 40, 4));
        // Ten do not: above, where there are fifteen.
        assert_eq!(anchored(at, 40, 10, BOUNDS), Rect::new(20, 10, 40, 10));
        // Taller than either side: the side with more room, cut to it.
        assert_eq!(anchored(at, 40, 30, BOUNDS), Rect::new(20, 5, 40, 15));
    }

    #[test]
    fn capped_by_the_room_below() {
        let at = Rect::new(20, 5, 8, 1);
        assert_eq!(anchored(at, 40, 50, BOUNDS), Rect::new(20, 6, 40, 19));
    }

    #[test]
    fn the_frame_is_opaque_with_its_title_and_hint_on_the_borders() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 6));
        for y in 0..6 {
            buf.set_string(0, y, "x".repeat(30), Style::new());
        }
        let inner = frame(
            &mut buf,
            Rect::new(2, 1, 20, 4),
            &[(" name ".into(), Style::new())],
            &[(" Esc a very long hint ".into(), Style::new())],
        );
        assert_eq!(inner, Rect::new(3, 2, 18, 2));
        let row = |y: u16| (0..30).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!(row(1), format!("xx\u{256d}\u{2500} name {}\u{256e}xxxxxxxx", "\u{2500}".repeat(11)));
        assert_eq!(row(2), "xx\u{2502}                  \u{2502}xxxxxxxx");
        // The hint is cut where the border ends, its corner kept.
        assert!(row(4).starts_with("xx\u{2570}\u{2500} Esc a very"), "{}", row(4));
        assert!(row(4).ends_with("\u{2500}\u{256f}xxxxxxxx"), "{}", row(4));
    }
}
