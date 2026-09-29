use super::*;

fn rgb_of(c: Color) -> (f64, f64, f64) {
    let Color::Rgb(r, g, b) = c else { panic!("{c:?} is not RGB") };
    (f64::from(r), f64::from(g), f64::from(b))
}

/// WCAG contrast ratio of two colors.
fn contrast(a: Color, b: Color) -> f64 {
    let lum = |c: Color| {
        let (r, g, b) = rgb_of(c);
        let ch = |v: f64| {
            let v = v / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The RGB of xterm-256 color `i` (16..=255: the 6×6×6 cube and the 24 grays).
fn xterm_rgb(i: u8) -> Color {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        16..=231 => {
            let n = i - 16;
            Color::Rgb(LEVELS[usize::from(n / 36)], LEVELS[usize::from(n / 6 % 6)], LEVELS[usize::from(n % 6)])
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            Color::Rgb(v, v, v)
        }
        _ => panic!("not a cube or gray color: {i}"),
    }
}

/// The xterm-256 color a terminal without truecolor shows for `c` (nearest of the 6×6×6
/// cube and the 24 grays).
fn xterm256(c: Color) -> u8 {
    const LEVELS: [f64; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
    let (r, g, b) = rgb_of(c);
    let d = |x: (f64, f64, f64)| (x.0 - r).powi(2) + (x.1 - g).powi(2) + (x.2 - b).powi(2);
    let mut best = (f64::MAX, 0u8);
    for (i, &lr) in LEVELS.iter().enumerate() {
        for (j, &lg) in LEVELS.iter().enumerate() {
            for (k, &lb) in LEVELS.iter().enumerate() {
                let dist = d((lr, lg, lb));
                if dist < best.0 {
                    best = (dist, (16 + 36 * i + 6 * j + k) as u8);
                }
            }
        }
    }
    for i in 0..24u8 {
        let v = 8.0 + 10.0 * f64::from(i);
        let dist = d((v, v, v));
        if dist < best.0 {
            best = (dist, 232 + i);
        }
    }
    best.1
}

const TEXT: [Color; 10] = [FG, FG_MUTED, FG_DIM, ACCENT, ACCENT_WARM, SUCCESS, WARNING, ERROR, SYN_KEYWORD, SYN_STRING];

#[test]
fn dimmed_screen_stays_readable_in_truecolor_and_256_colors() {
    let back = dim_bg(BG);
    assert!(contrast(dim_fg(FG), back) >= 4.5, "body text: {:.2}", contrast(dim_fg(FG), back));
    for c in TEXT {
        assert!(contrast(dim_fg(c), back) > 1.5, "{c:?}: {:.2}", contrast(dim_fg(c), back));
        assert_ne!(xterm256(dim_fg(c)), xterm256(back), "{c:?} vanishes into the backdrop with 256 colors");
    }
    // Dimming is visible, and a dialog's surface stands out from the dimmed screen.
    assert_ne!(xterm256(dim_fg(FG)), xterm256(FG));
    assert!(contrast(dim_fg(FG), back) < contrast(FG, BG));
    for surface in [SURFACE, SELECTION_BG] {
        assert_ne!(xterm256(surface), xterm256(back), "{surface:?}");
        assert!(contrast(surface, back) > contrast(SURFACE, BG), "{surface:?}");
    }
}

#[test]
fn dim_area_keeps_symbols_and_handles_default_colors() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
    buf.set_string(0, 0, "ab", Style::new().fg(ACCENT).bg(SURFACE));
    dim_area(&mut buf, Rect::new(1, 0, 5, 1));
    assert_eq!((buf[(0, 0)].symbol(), buf[(0, 0)].fg), ("a", ACCENT), "outside the area");
    assert_eq!((buf[(1, 0)].symbol(), buf[(1, 0)].fg, buf[(1, 0)].bg), ("b", dim_fg(ACCENT), dim_bg(SURFACE)));
    // A cell with the terminal's default colors gets the dimmed theme colors.
    assert_eq!((buf[(2, 0)].fg, buf[(2, 0)].bg), (dim_fg(FG), dim_bg(BG)));
}

#[test]
fn key_marks_read_on_the_background_and_differ_in_256_colors() {
    let keys = [KEY_PK, KEY_FK, KEY_UQ];
    for c in keys {
        for bg in [BG, SURFACE, SURFACE_ALT] {
            assert!(contrast(c, bg) >= 4.5, "{c:?} on {bg:?}: {:.2}", contrast(c, bg));
        }
        assert_ne!(xterm256(dim_fg(c)), xterm256(dim_bg(BG)), "{c:?} vanishes behind a dialog");
    }
    let mapped: Vec<u8> = keys.iter().map(|&c| xterm256(c)).collect();
    assert!(mapped[0] != mapped[1] && mapped[1] != mapped[2] && mapped[0] != mapped[2], "{mapped:?}");
}

#[test]
fn profile_colors_read_on_the_background_and_differ() {
    use datarig_core::profile::color::{NAMES, ProfileColor};
    assert_eq!(PROFILE_COLORS.len(), NAMES.len());
    for (i, &c) in PROFILE_COLORS.iter().enumerate() {
        assert!(contrast(c, BG) >= 4.5, "{} on BG: {:.2}", NAMES[i], contrast(c, BG));
        assert_eq!(profile_color(ProfileColor::Named(i as u8)), c);
    }
    for i in 0..PROFILE_COLORS.len() {
        for j in i + 1..PROFILE_COLORS.len() {
            let (a, b) = (rgb_of(PROFILE_COLORS[i]), rgb_of(PROFILE_COLORS[j]));
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
            assert!(d >= 40.0, "{} and {} are too close ({d:.0})", NAMES[i], NAMES[j]);
        }
    }
    assert_eq!(profile_color(ProfileColor::Hex(1, 2, 3)), Color::Rgb(1, 2, 3));
}

#[test]
fn mode_badges_read_and_differ_in_truecolor_and_256_colors() {
    let badges = [MODE_NORMAL, MODE_INSERT, MODE_VISUAL, MODE_COMMAND, MODE_NEUTRAL];
    for bg in badges {
        assert!(contrast(MODE_FG, bg) >= 4.5, "{bg:?}: {:.2}", contrast(MODE_FG, bg));
        let (fg256, bg256) = (xterm_rgb(xterm256(MODE_FG)), xterm_rgb(xterm256(bg)));
        assert!(contrast(fg256, bg256) >= 4.5, "{bg:?} in 256 colors: {:.2}", contrast(fg256, bg256));
        assert_ne!(xterm256(bg), xterm256(SURFACE), "{bg:?} vanishes into the status bar");
    }
    let mapped: Vec<u8> = badges.iter().map(|&c| xterm256(c)).collect();
    for i in 0..mapped.len() {
        for j in i + 1..mapped.len() {
            assert_ne!(mapped[i], mapped[j], "{:?} and {:?} look the same in 256 colors", badges[i], badges[j]);
        }
    }
}

#[test]
fn the_statement_bar_stays_visible_where_the_tint_does_not() {
    for bg in [BG, CURRENT_STMT_BG, CURSOR_LINE_BG] {
        // A non-text mark: 3:1 (WCAG 1.4.11).
        assert!(contrast(CURRENT_STMT_BAR, bg) >= 3.0, "{bg:?}: {:.2}", contrast(CURRENT_STMT_BAR, bg));
        assert_ne!(xterm256(CURRENT_STMT_BAR), xterm256(bg), "{bg:?} in 256 colors");
    }
    // The tint is faint: close to the background, below the cursor line's.
    assert!(contrast(CURRENT_STMT_BG, BG) < contrast(CURSOR_LINE_BG, BG));
}
