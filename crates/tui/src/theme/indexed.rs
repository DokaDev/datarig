//! Terminals of 256 colors: a 24-bit color is drawn as the nearest of the xterm palette (by the
//! terminal, or by a multiplexer in between), so a faint tint can become the very background it
//! sits on and its cue is gone. [`Theme::for_256`] gives such tints a palette color of their
//! own, the nearest one that is not a background they sit on.

use super::Theme;
use ratatui::style::{Color, Style};

/// How many colors the terminal draws, as its environment says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit colors (or nothing says otherwise): drawn as they are.
    #[default]
    TrueColor,
    /// The xterm palette of 256 colors.
    Indexed256,
}

impl ColorDepth {
    /// From `COLORTERM` and `TERM`: `truecolor` or `24bit` in `COLORTERM` is 24-bit; else a
    /// `TERM` that names 256 colors (`xterm-256color`, `tmux-256color`) is the palette. Anything
    /// else is left as 24-bit: colors are only changed where the palette is known.
    pub fn of(colorterm: Option<&str>, term: Option<&str>) -> Self {
        let colorterm = colorterm.unwrap_or("").to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            return Self::TrueColor;
        }
        if term.is_some_and(|t| t.to_ascii_lowercase().contains("256color")) {
            return Self::Indexed256;
        }
        Self::TrueColor
    }

    /// [`Self::of`] the process's environment.
    pub fn from_env() -> Self {
        let colorterm = std::env::var("COLORTERM").ok();
        let term = std::env::var("TERM").ok();
        Self::of(colorterm.as_deref(), term.as_deref())
    }
}

/// The levels of each channel in the palette's 6×6×6 cube (indexes 16 to 231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The RGB of palette index `i` (16 to 255: the cube, then the 24 grays).
fn rgb_of(i: u8) -> (u8, u8, u8) {
    match i {
        16..=231 => {
            let n = i - 16;
            (CUBE[usize::from(n / 36)], CUBE[usize::from(n / 6 % 6)], CUBE[usize::from(n % 6)])
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
        _ => (0, 0, 0),
    }
}

fn distance((r, g, b): (u8, u8, u8), (r2, g2, b2): (u8, u8, u8)) -> i32 {
    let d = |a: u8, b: u8| i32::from(a) - i32::from(b);
    d(r, r2).pow(2) + d(g, g2).pow(2) + d(b, b2).pow(2)
}

/// The palette index a terminal of 256 colors draws `rgb` as: the nearest of the cube colors
/// and the grays (16 to 255; the first 16 are the terminal's own and not used for it). The
/// lowest index wins a tie.
pub fn nearest(rgb: (u8, u8, u8)) -> u8 {
    (16..=255u8).min_by_key(|i| distance(rgb, rgb_of(*i))).unwrap_or(16)
}

/// The palette index `c` is drawn as, for an RGB color.
fn index_of(c: Color) -> Option<u8> {
    match c {
        Color::Rgb(r, g, b) => Some(nearest((r, g, b))),
        Color::Indexed(i) => Some(i),
        _ => None,
    }
}

/// `tint`'s fill drawn apart from every color of `under` on a terminal of 256 colors: as it is
/// when its palette color differs from theirs, else the palette color nearest to it that is
/// none of theirs, on the same side of the first one as the tint (lighter on a dark background,
/// darker on a light one). A tint with no RGB fill, or on an unknown background, stays.
fn apart(tint: Style, under: &[Color]) -> Style {
    let Some(Color::Rgb(r, g, b)) = tint.bg else { return tint };
    let taken: Option<Vec<u8>> = under.iter().map(|c| index_of(*c)).collect();
    let Some(taken) = taken else { return tint };
    if !taken.contains(&nearest((r, g, b))) {
        return tint;
    }
    let light = |(r, g, b): (u8, u8, u8)| i32::from(r) + i32::from(g) + i32::from(b);
    let base = light(rgb_of(taken[0]));
    let side = (light((r, g, b)) - base).signum();
    let free = |i: &u8| !taken.contains(i);
    let near = |i: &u8| distance((r, g, b), rgb_of(*i));
    let same_side = (16..=255u8).filter(free).filter(|i| side == 0 || (light(rgb_of(*i)) - base).signum() == side);
    let pick = same_side.min_by_key(near).or_else(|| (16..=255u8).filter(free).min_by_key(near));
    match pick {
        Some(i) => tint.bg(Color::Indexed(i)),
        None => tint,
    }
}

impl Theme {
    /// This theme for a terminal of 256 colors: each tint that marks where the cursor or the
    /// selection is, and would be drawn as the background it sits on, gets a palette color of
    /// its own (the nearest one apart). Everything else stays as it is: the terminal maps it.
    pub fn for_256(&self) -> Theme {
        let mut th = self.clone();
        let (bg, alt, surface) = (self.bg, self.surface_alt, self.surface);
        th.grid_cursor_row = apart(self.grid_cursor_row, &[bg, alt]);
        th.grid_cursor_header = apart(self.grid_cursor_header, &[surface]);
        th.cursor_line = apart(self.cursor_line, &[bg]);
        th.current_stmt = apart(self.current_stmt, &[bg]);
        th.running_stmt = apart(self.running_stmt, &[bg]);
        th.hover = apart(self.hover, &[bg, alt]);
        // The cursor's cell and a range sit on the cursor row's tint too; a range also tints the
        // headers of its columns, next to the header's surface and the cursor column's header.
        let row = th.grid_cursor_row.bg.unwrap_or(bg);
        let header = th.grid_cursor_header.bg.unwrap_or(surface);
        th.selection = apart(self.selection, &[bg, alt, row]);
        th.range = apart(self.range, &[bg, alt, row, surface, header]);
        th
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_says_how_many_colors() {
        use ColorDepth::*;
        assert_eq!(ColorDepth::of(Some("truecolor"), Some("xterm-256color")), TrueColor);
        assert_eq!(ColorDepth::of(Some("24bit"), Some("tmux-256color")), TrueColor);
        assert_eq!(ColorDepth::of(None, Some("xterm-256color")), Indexed256);
        assert_eq!(ColorDepth::of(Some(""), Some("screen-256color")), Indexed256);
        assert_eq!(ColorDepth::of(None, Some("xterm")), TrueColor, "not known: left as it is");
        assert_eq!(ColorDepth::of(None, None), TrueColor);
    }

    #[test]
    fn nearest_follows_the_xterm_palette() {
        assert_eq!(nearest((0, 0, 0)), 16);
        assert_eq!(nearest((255, 255, 255)), 231);
        assert_eq!(nearest((255, 0, 0)), 196);
        assert_eq!(nearest((0x26, 0x26, 0x26)), 235);
        assert_eq!(nearest((0x80, 0x80, 0x80)), 244);
        // Not a palette color: the nearest gray, not the one below it (the dark theme's
        // alternate rows and its cursor row's tint).
        assert_eq!(nearest((0x20, 0x24, 0x2C)), 235);
        assert_eq!(nearest((0x17, 0x1F, 0x23)), 234);
        for i in 16..=255u8 {
            assert_eq!(index_of(Color::Indexed(i)), Some(i));
            let back = nearest(rgb_of(i));
            assert_eq!(rgb_of(back), rgb_of(i), "index {i}");
        }
    }

    /// On every built-in theme, the cursor and selection tints a 256-color terminal draws stay
    /// apart from the backgrounds they sit on; tints already apart are not changed.
    #[test]
    fn every_builtin_keeps_its_cursor_cues_on_256_colors() {
        for name in crate::theme::NAMES {
            for bgd in [crate::theme::Background::Dark, crate::theme::Background::Light] {
                let th = crate::theme::resolve(name, bgd, None).unwrap();
                let t = th.for_256();
                let ix = |c: Option<Color>| c.and_then(index_of);
                let pairs = [
                    ("grid_cursor_row", t.grid_cursor_row.bg, vec![t.bg, t.surface_alt]),
                    ("grid_cursor_header", t.grid_cursor_header.bg, vec![t.surface]),
                    ("cursor_line", t.cursor_line.bg, vec![t.bg]),
                    ("current_stmt", t.current_stmt.bg, vec![t.bg]),
                    ("selection", t.selection.bg, vec![t.bg, t.surface_alt, t.grid_cursor_row.bg.unwrap_or(t.bg)]),
                    (
                        "range",
                        t.range.bg,
                        vec![
                            t.bg,
                            t.surface_alt,
                            t.grid_cursor_row.bg.unwrap_or(t.bg),
                            t.surface,
                            t.grid_cursor_header.bg.unwrap_or(t.surface),
                        ],
                    ),
                ];
                for (token, tint, under) in pairs {
                    let (Some(tint), true) = (ix(tint), under.iter().all(|c| index_of(*c).is_some())) else {
                        continue;
                    };
                    for c in under {
                        assert_ne!(Some(tint), index_of(c), "{name}: {token} on {c:?}");
                    }
                }
                if let (Some(Color::Rgb(r, g, b)), Some(Color::Rgb(..))) = (th.grid_cursor_row.bg, Some(th.bg))
                    && ![th.bg, th.surface_alt].iter().any(|c| index_of(*c) == Some(nearest((r, g, b))))
                {
                    assert_eq!(t.grid_cursor_row, th.grid_cursor_row, "{name}: apart already, unchanged");
                }
            }
        }
    }

    /// The tints that would vanish move away from the background in their own direction: the
    /// high-contrast theme's cursor row (drawn as the alternate rows' gray) gets a lighter
    /// color, the light theme's a darker one.
    #[test]
    fn a_tint_that_would_vanish_moves_its_own_way() {
        let light = |i: u8| {
            let (r, g, b) = rgb_of(i);
            u32::from(r) + u32::from(g) + u32::from(b)
        };
        for (th, lighter) in [(crate::theme::HIGH_CONTRAST, true), (crate::theme::LIGHT, false)] {
            let Some(Color::Rgb(r, g, b)) = th.grid_cursor_row.bg else { panic!("rgb") };
            let Color::Rgb(br, bgg, bb) = th.bg else { panic!("rgb") };
            let (Color::Rgb(ar, ag, ab), bg) = (th.surface_alt, nearest((br, bgg, bb))) else { panic!("rgb") };
            assert!([bg, nearest((ar, ag, ab))].contains(&nearest((r, g, b))), "it would vanish");
            let Some(Color::Indexed(i)) = th.for_256().grid_cursor_row.bg else { panic!("a palette color") };
            assert_eq!(light(i) > light(bg), lighter, "index {i} on {bg}");
        }
    }

    #[test]
    fn a_theme_without_rgb_colors_is_unchanged() {
        let th = crate::theme::TERMINAL;
        let t = th.for_256();
        assert_eq!((t.grid_cursor_row, t.cursor_line, t.selection), (th.grid_cursor_row, th.cursor_line, th.selection));
    }
}
