use super::*;
use crate::icons::KeyMark;

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

/// The foreground color of a style token.
fn fg(s: Style) -> Color {
    s.fg.expect("a style token with a foreground")
}

/// The background color of a style token.
fn bg(s: Style) -> Color {
    s.bg.expect("a style token with a background")
}

/// The built-in themes that draw with RGB colors and dim by blending (the contrast checks
/// below are about RGB).
fn rgb_themes() -> impl Iterator<Item = (&'static str, &'static Theme)> {
    BUILTINS.iter().copied().filter(|(_, th)| matches!(th.dim, Dim::Blend { .. }))
}

fn text(th: &Theme) -> [Color; 10] {
    [
        th.fg,
        th.fg_muted,
        th.fg_dim,
        th.accent,
        th.accent_warm,
        th.success,
        th.warning,
        th.error,
        fg(th.syn_keyword),
        fg(th.syn_string),
    ]
}

#[test]
fn dark_is_a_builtin() {
    assert!(BUILTINS.iter().any(|&(name, th)| name == "dark" && *th == DARK));
    let names: Vec<&str> = BUILTINS.iter().map(|(n, _)| *n).collect();
    let mut sorted = names.clone();
    sorted.dedup();
    assert_eq!(names, sorted, "names are unique");
}

#[test]
fn the_current_theme_is_dark_until_a_scope_sets_one_and_comes_back_after() {
    assert_eq!(*cur(), DARK);
    let mut other = DARK;
    other.fg = Color::Rgb(1, 2, 3);
    {
        let _outer = scope(Arc::new(other.clone()));
        assert_eq!(cur().fg, Color::Rgb(1, 2, 3));
        {
            let _inner = scope(Arc::new(DARK));
            assert_eq!(*cur(), DARK);
        }
        assert_eq!(cur().fg, Color::Rgb(1, 2, 3));
        // Another thread keeps its own theme.
        std::thread::spawn(|| assert_eq!(*cur(), DARK)).join().unwrap();
    }
    assert_eq!(*cur(), DARK);
}

#[test]
fn dimmed_screen_stays_readable_in_truecolor_and_256_colors() {
    for (name, th) in rgb_themes() {
        let back = th.dim_bg(th.bg);
        let body = contrast(th.dim_fg(th.fg), back);
        assert!(body >= 4.5, "{name}: body text: {body:.2}");
        for c in text(th) {
            assert!(contrast(th.dim_fg(c), back) > 1.5, "{name}: {c:?}: {:.2}", contrast(th.dim_fg(c), back));
            assert_ne!(
                xterm256(th.dim_fg(c)),
                xterm256(back),
                "{name}: {c:?} vanishes into the backdrop with 256 colors"
            );
        }
        // Dimming is visible, and a dialog's surface stands out from the dimmed screen.
        assert_ne!(xterm256(th.dim_fg(th.fg)), xterm256(th.fg), "{name}");
        assert!(body < contrast(th.fg, th.bg), "{name}");
        for surface in [th.surface, bg(th.selection)] {
            assert_ne!(xterm256(surface), xterm256(back), "{name}: {surface:?}");
            assert!(contrast(surface, back) > contrast(th.surface, th.bg), "{name}: {surface:?}");
        }
    }
}

#[test]
fn dim_area_keeps_symbols_and_handles_default_colors() {
    for (name, th) in BUILTINS.iter().copied() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(0, 0, "ab", Style::new().fg(th.accent).bg(th.surface));
        th.dim_area(&mut buf, Rect::new(1, 0, 5, 1));
        let outside = &buf[(0, 0)];
        assert_eq!((outside.symbol(), outside.fg, outside.modifier), ("a", th.accent, Modifier::empty()), "{name}");
        match th.dim {
            Dim::Blend { .. } => {
                let b = &buf[(1, 0)];
                assert_eq!((b.symbol(), b.fg, b.bg), ("b", th.dim_fg(th.accent), th.dim_bg(th.surface)), "{name}");
                // A cell with the terminal's default colors gets the dimmed theme colors.
                assert_eq!((buf[(2, 0)].fg, buf[(2, 0)].bg), (th.dim_fg(th.fg), th.dim_bg(th.bg)), "{name}");
            }
            Dim::Modifier => {
                let b = &buf[(1, 0)];
                assert_eq!((b.symbol(), b.fg, b.bg), ("b", th.accent, th.surface), "{name}");
                assert!(b.modifier.contains(Modifier::DIM) && buf[(2, 0)].modifier.contains(Modifier::DIM), "{name}");
            }
        }
    }
}

#[test]
fn key_marks_read_on_the_background_and_differ_in_256_colors() {
    for (name, th) in rgb_themes() {
        let keys = [th.key_pk, th.key_fk, th.key_uq];
        for c in keys {
            for bg in [th.bg, th.surface, th.surface_alt] {
                assert!(contrast(c, bg) >= 4.5, "{name}: {c:?} on {bg:?}: {:.2}", contrast(c, bg));
            }
            assert_ne!(xterm256(th.dim_fg(c)), xterm256(th.dim_bg(th.bg)), "{name}: {c:?} vanishes behind a dialog");
        }
        let mapped: Vec<u8> = keys.iter().map(|&c| xterm256(c)).collect();
        assert!(mapped[0] != mapped[1] && mapped[1] != mapped[2] && mapped[0] != mapped[2], "{name}: {mapped:?}");
        assert_eq!(keys, [KeyMark::Pk, KeyMark::Fk, KeyMark::Uq].map(|k| th.key_color(k)), "{name}");
    }
}

#[test]
fn profile_colors_read_on_the_background_and_differ() {
    use datarig_core::profile::color::{NAMES, ProfileColor};
    assert_eq!(PROFILE_COLORS.len(), NAMES.len());
    for (i, &c) in PROFILE_COLORS.iter().enumerate() {
        for (name, th) in rgb_themes() {
            assert!(contrast(c, th.bg) >= 4.5, "{name}: {} on bg: {:.2}", NAMES[i], contrast(c, th.bg));
        }
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
fn profile_colors_do_not_follow_the_theme() {
    use datarig_core::profile::color::ProfileColor;
    let mut other = DARK;
    other.bg = Color::Rgb(250, 250, 250);
    let before = profile_color(ProfileColor::Named(0));
    let _theme = scope(Arc::new(other));
    assert_eq!(profile_color(ProfileColor::Named(0)), before);
}

#[test]
fn mode_badges_read_and_differ_in_truecolor_and_256_colors() {
    for (name, th) in rgb_themes() {
        let badges = [th.mode_normal, th.mode_insert, th.mode_visual, th.mode_command, th.mode_neutral];
        for bg in badges {
            assert!(contrast(th.mode_fg, bg) >= 4.5, "{name}: {bg:?}: {:.2}", contrast(th.mode_fg, bg));
            let (fg256, bg256) = (xterm_rgb(xterm256(th.mode_fg)), xterm_rgb(xterm256(bg)));
            assert!(contrast(fg256, bg256) >= 4.5, "{name}: {bg:?} in 256 colors: {:.2}", contrast(fg256, bg256));
            assert_ne!(xterm256(bg), xterm256(th.surface), "{name}: {bg:?} vanishes into the status bar");
        }
        let mapped: Vec<u8> = badges.iter().map(|&c| xterm256(c)).collect();
        for i in 0..mapped.len() {
            for j in i + 1..mapped.len() {
                assert_ne!(
                    mapped[i], mapped[j],
                    "{name}: {:?} and {:?} look the same in 256 colors",
                    badges[i], badges[j]
                );
            }
        }
    }
}

#[test]
fn the_statement_bar_stays_visible_where_the_tint_does_not() {
    for (name, th) in rgb_themes() {
        let (tint, cursor_line) = (bg(th.current_stmt), bg(th.cursor_line));
        for bg in [th.bg, tint, cursor_line] {
            // A non-text mark: 3:1 (WCAG 1.4.11).
            let c = contrast(th.current_stmt_bar, bg);
            assert!(c >= 3.0, "{name}: {bg:?}: {c:.2}");
            assert_ne!(xterm256(th.current_stmt_bar), xterm256(bg), "{name}: {bg:?} in 256 colors");
        }
        // The tint is faint: close to the background, below the cursor line's.
        assert!(contrast(tint, th.bg) < contrast(cursor_line, th.bg), "{name}");
    }
}

#[test]
fn marks_carry_a_modifier_and_stand_apart_from_body_text() {
    for (name, th) in BUILTINS.iter().copied() {
        for (token, s) in [("read_only_mark", th.read_only_mark), ("danger_mark", th.danger_mark)] {
            assert!(!s.add_modifier.is_empty(), "{name}: {token} shows without color");
            assert_ne!(s.fg, Some(th.fg), "{name}: {token} looks like body text");
        }
        assert_ne!(th.read_only_mark, th.danger_mark, "{name}");
    }
}

#[test]
fn syntax_styles_are_the_theme_tokens() {
    use datarig_core::sql::lexer::Tok;
    let th = DARK;
    assert_eq!(th.syntax(Tok::Keyword, false), Style::new().fg(fg(th.syn_keyword)).add_modifier(Modifier::BOLD));
    assert_eq!(th.syntax(Tok::LineComment, false), Style::new().fg(fg(th.syn_comment)).add_modifier(Modifier::ITALIC));
    assert_eq!(th.syntax(Tok::Keyword, true), th.syn_function);
    assert_eq!(th.syntax(Tok::Whitespace, false), Style::new().fg(th.fg));
}
