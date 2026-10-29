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

/// A background is dark when light text reads on it better than dark text.
fn dark_background(th: &Theme) -> bool {
    contrast(th.bg, Color::Rgb(255, 255, 255)) > contrast(th.bg, Color::Rgb(0, 0, 0))
}

#[test]
fn every_name_is_a_builtin_or_a_family_of_two_builtins() {
    assert!(BUILTINS.iter().any(|&(name, th)| name == "dark" && *th == DARK));
    assert!(BUILTINS.iter().any(|&(name, th)| name == "terminal" && *th == TERMINAL));
    let mut all: Vec<&str> = BUILTINS.iter().map(|(n, _)| *n).chain(FAMILIES.iter().map(|f| f.name)).collect();
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "names are unique");
    let mut names = NAMES.to_vec();
    names.sort();
    assert_eq!(names, all, "NAMES lists every built-in and every family once");
    for f in FAMILIES {
        let (light, dark) =
            (builtin(f.light, Background::Unknown).unwrap(), builtin(f.dark, Background::Unknown).unwrap());
        assert!(!dark_background(light) && dark_background(dark), "{}", f.name);
        assert_eq!(builtin(f.name, Background::Light), Some(light), "{}", f.name);
        assert_eq!(builtin(f.name, Background::Dark), Some(dark), "{}", f.name);
        assert_eq!(builtin(f.name, Background::Unknown), Some(dark), "{}: no answer is dark", f.name);
        assert_eq!(builtin(f.light, Background::Dark), Some(light), "a variant's name pins it");
    }
    assert_eq!(builtin("nope", Background::Dark), None);
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

/// Profile colors are the same in every theme (a profile's color is the user's choice): they
/// read as text on `dark`'s background and at least as marks (3:1) on every other dark one.
#[test]
fn profile_colors_read_on_the_background_and_differ() {
    use datarig_core::profile::color::{NAMES, ProfileColor};
    assert_eq!(PROFILE_COLORS.len(), NAMES.len());
    for (i, &c) in PROFILE_COLORS.iter().enumerate() {
        assert!(contrast(c, DARK.bg) >= 4.5, "{} on dark: {:.2}", NAMES[i], contrast(c, DARK.bg));
        for (name, th) in rgb_themes().filter(|(_, th)| dark_background(th)) {
            assert!(contrast(c, th.bg) >= 3.0, "{name}: {} on bg: {:.2}", NAMES[i], contrast(c, th.bg));
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
    let before: Vec<Color> = (0..12).map(|i| profile_color(ProfileColor::Named(i))).collect();
    for (_, th) in BUILTINS {
        let _theme = scope(Arc::new((*th).clone()));
        let now: Vec<Color> = (0..12).map(|i| profile_color(ProfileColor::Named(i))).collect();
        assert_eq!(now, before);
    }
}

#[test]
fn body_and_muted_text_read_on_every_surface() {
    for (name, th) in rgb_themes() {
        for bg in [th.bg, th.surface, th.surface_alt, bg(th.selection), bg(th.cursor_line)] {
            assert!(contrast(th.fg, bg) >= 4.5, "{name}: body text on {bg:?}: {:.2}", contrast(th.fg, bg));
        }
        for bg in [th.bg, th.surface, th.surface_alt] {
            assert!(contrast(th.fg_muted, bg) >= 3.0, "{name}: muted on {bg:?}: {:.2}", contrast(th.fg_muted, bg));
            for c in [th.accent, th.accent_warm, th.success, th.warning, th.error] {
                assert!(contrast(c, bg) >= 3.0, "{name}: {c:?} on {bg:?}: {:.2}", contrast(c, bg));
            }
        }
    }
}

/// The theme of the terminal's colors is about roles: which ANSI color, not which RGB.
#[test]
fn the_terminal_theme_uses_the_terminal_colors_by_role() {
    let th = &TERMINAL;
    assert_eq!((th.fg, th.bg, th.surface), (Color::Reset, Color::Reset, Color::Reset), "its own text and background");
    assert_eq!(th.warning, Color::Yellow);
    assert_eq!(th.error, Color::Red);
    assert_eq!(th.danger_mark.fg, Some(Color::Red), "danger is the error role");
    assert_eq!(th.success, Color::Green);
    // Selection shows without any color: reversed text.
    assert!(th.selection.add_modifier.contains(Modifier::REVERSED));
    assert_eq!((th.selection.fg, th.selection.bg), (None, None));
    // The statement tint is off, its bar stays.
    assert_eq!(th.current_stmt, Style::new());
    assert_ne!(th.current_stmt_bar, Color::Reset);
    // The running statement shows without color, apart from the run target, the cursor line
    // and the selection (bold text); its bar is another color, and its hint is muted text that
    // shows without color too.
    assert!(!th.running_stmt.add_modifier.is_empty(), "{:?}", th.running_stmt);
    for other in [th.current_stmt, th.cursor_line, th.selection] {
        assert_ne!(th.running_stmt.add_modifier, other.add_modifier, "{other:?}");
    }
    assert!(!matches!(th.running_stmt_bar, Color::Reset | Color::Rgb(..) | Color::Indexed(_)));
    assert_ne!(th.running_stmt_bar, th.current_stmt_bar);
    assert_eq!(th.run_hint.fg, Some(th.fg_muted));
    assert!(th.run_hint.add_modifier.contains(Modifier::ITALIC));
    assert_eq!(th.dim, Dim::Modifier);
    // No text role looks like body text, and no mark is invisible without color.
    for c in [th.accent, th.accent_warm, th.success, th.warning, th.error, th.fg_muted, th.key_pk, th.key_fk, th.key_uq]
    {
        assert!(!matches!(c, Color::Reset | Color::Rgb(..) | Color::Indexed(_)), "{c:?}: an ANSI role");
    }
    for s in [th.range, th.cursor_line, th.read_only_mark, th.danger_mark, th.search_match, th.match_paren] {
        assert_ne!(s, Style::new(), "{s:?} shows");
    }
    assert!(th.range != th.selection && th.range != th.cursor_line && th.cursor_line != th.selection);
    let keys = [th.key_pk, th.key_fk, th.key_uq];
    assert!(keys[0] != keys[1] && keys[1] != keys[2] && keys[0] != keys[2]);
    let badges = [th.mode_normal, th.mode_insert, th.mode_visual, th.mode_command];
    for (i, a) in badges.iter().enumerate() {
        assert_ne!(*a, th.mode_fg);
        assert!(badges[i + 1..].iter().all(|b| b != a), "{a:?} twice");
    }
}

#[test]
fn warning_read_only_and_danger_stand_apart_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let (ro, danger) = (th.read_only_mark.fg, th.danger_mark.fg);
        assert!(ro.is_some() && danger.is_some(), "{name}");
        assert!(Some(th.warning) != ro && Some(th.warning) != danger && ro != danger, "{name}");
        assert_ne!(th.warning, th.error, "{name}");
        for s in [th.read_only_mark, th.danger_mark] {
            assert!(s.add_modifier.intersects(Modifier::BOLD | Modifier::REVERSED), "{name}: {s:?}");
        }
    }
}

#[test]
fn every_token_of_a_theme_can_be_set_by_name() {
    // Listing every field (no `..`): a token added to `Theme` fails to compile here until it is
    // in the token table too.
    let Theme {
        bg: _,
        surface: _,
        surface_alt: _,
        border: _,
        accent: _,
        accent_warm: _,
        fg: _,
        fg_muted: _,
        fg_dim: _,
        selection: _,
        range: _,
        cursor_line: _,
        hover: _,
        current_stmt: _,
        current_stmt_bar: _,
        running_stmt: _,
        running_stmt_bar: _,
        run_hint: _,
        success: _,
        warning: _,
        error: _,
        null_fg: _,
        syn_keyword: _,
        syn_function: _,
        syn_string: _,
        syn_number: _,
        syn_comment: _,
        syn_operator: _,
        syn_identifier: _,
        syn_quoted_ident: _,
        syn_exec_comment: _,
        key_pk: _,
        key_fk: _,
        key_uq: _,
        mode_normal: _,
        mode_insert: _,
        mode_visual: _,
        mode_command: _,
        mode_fg: _,
        search_match: _,
        grid_match: _,
        grid_match_current: _,
        match_paren: _,
        read_only_mark: _,
        danger_mark: _,
        plan_hot: _,
        plan_misestimate: _,
        size_warm: _,
        size_hot: _,
        size_mild: _,
        size_heated: _,
        yank_flash: _,
        tab_active: _,
        chart_1: _,
        chart_2: _,
        chart_3: _,
        chart_4: _,
        chart_5: _,
        chart_6: _,
        dim: _,
    } = DARK;
    // Every field above but `dim`.
    assert_eq!(COLOR_TOKENS.len() + STYLE_TOKENS.len(), 59);
    let mut th = DARK;
    for (i, name) in COLOR_TOKENS.iter().enumerate() {
        *color_token(&mut th, name).unwrap() = Color::Indexed(i as u8);
    }
    for (i, name) in STYLE_TOKENS.iter().enumerate() {
        *style_token(&mut th, name).unwrap() = Style::new().bg(Color::Indexed(100 + i as u8));
    }
    assert_eq!((th.bg, th.mode_fg), (Color::Indexed(0), Color::Indexed(22)));
    assert_eq!(th.yank_flash, Style::new().bg(Color::Indexed(104)));
    assert_eq!(th.plan_misestimate, Style::new().bg(Color::Indexed(122)));
    assert_eq!(
        (th.size_warm, th.size_hot),
        (Style::new().bg(Color::Indexed(123)), Style::new().bg(Color::Indexed(124)))
    );
    assert_eq!(
        (th.size_mild, th.size_heated),
        (Style::new().bg(Color::Indexed(125)), Style::new().bg(Color::Indexed(126)))
    );
    assert_eq!(th.tab_active, Style::new().bg(Color::Indexed(127)));
    assert_eq!(th.chart(), [23, 24, 25, 26, 27, 28].map(Color::Indexed));
    assert!(color_token(&mut th, "selection").is_none() && style_token(&mut th, "bg").is_none());
}

#[test]
fn file_colors_map_to_the_terminal_palette_or_rgb() {
    use datarig_core::theme::ColorSpec;
    assert_eq!(color(ColorSpec::Default), Color::Reset);
    assert_eq!(color(ColorSpec::Ansi(1)), Color::Red);
    assert_eq!(color(ColorSpec::Ansi(7)), Color::Gray, "white is palette color 7");
    assert_eq!(color(ColorSpec::Ansi(8)), Color::DarkGray, "bright-black is palette color 8");
    assert_eq!(color(ColorSpec::Ansi(15)), Color::White);
    assert_eq!(color(ColorSpec::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
}

/// A scratch `themes/` directory, removed when dropped.
struct ThemesDir(std::path::PathBuf);

impl ThemesDir {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("datarig-theme-{}-{tag}", std::process::id())).join("themes");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        ThemesDir(d)
    }

    fn file(&self, name: &str, text: &str) -> &Self {
        std::fs::write(self.0.join(format!("{name}.toml")), text).unwrap();
        self
    }
}

impl Drop for ThemesDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

#[test]
fn a_theme_file_replaces_tokens_of_the_theme_it_extends() {
    use datarig_core::theme::{Problem, ThemeError};
    let d = ThemesDir::new("resolve");
    d.file(
        "mine",
        "extends = \"gruvbox\"\n[colors]\nwarning = \"bright-yellow\"\n[styles]\nselection = { bg = \"#102030\", modifiers = [\"bold\"] }\n",
    );
    let dir = Some(d.0.as_path());
    let light = resolve("mine", Background::Light, dir).unwrap();
    assert_eq!(light.warning, Color::LightYellow);
    assert_eq!(light.selection, Style::new().bg(Color::Rgb(0x10, 0x20, 0x30)).add_modifier(Modifier::BOLD));
    assert_eq!(light.fg, GRUVBOX_LIGHT.fg, "the rest is the base's; a family follows the background");
    assert_eq!(resolve("mine", Background::Unknown, dir).unwrap().fg, GRUVBOX_DARK.fg);
    // Without `extends`: the terminal theme.
    d.file("bare", "[colors]\naccent = \"#ff0000\"\n");
    let bare = resolve("bare", Background::Dark, dir).unwrap();
    assert_eq!(bare, Theme { accent: Color::Rgb(255, 0, 0), ..TERMINAL });
    // Built-in names never read a file; a file with such a name is an error, not ignored.
    assert_eq!(resolve("nord", Background::Dark, dir).unwrap(), NORD);
    d.file("nord", "");
    assert_eq!(resolve("nord", Background::Dark, dir), Err(ThemeError::Shadows { path: d.0.join("nord.toml") }));
    assert_eq!(
        names(dir),
        [NAMES.iter().map(|n| n.to_string()).collect::<Vec<_>>(), vec!["bare".into(), "mine".into()]].concat()
    );
    d.file("base", "\n\nextends = \"solarized\"\n");
    assert_eq!(
        resolve("base", Background::Dark, dir),
        Err(ThemeError::File {
            path: d.0.join("base.toml"),
            line: Some(3),
            problem: Problem::UnknownBase("solarized".into())
        })
    );
    assert!(matches!(resolve("gone", Background::Dark, dir), Err(ThemeError::Unknown { .. })));
    assert_eq!(resolve("mine", Background::Dark, None), Err(ThemeError::Unknown { name: "mine".into(), dir: None }));
    assert_eq!(resolve("terminal", Background::Dark, None).unwrap(), TERMINAL);
}

/// A theme file that leaves `tab_active` out keeps the fill of the theme it extends while its
/// background and accent are that theme's; with its own background or accent (a file written
/// before the token existed), the active tab is that accent blended into that background, so it
/// never keeps a fill made for other colors. A file that sets it keeps what it set.
#[test]
fn a_theme_file_without_the_active_tab_fill_gets_one_of_its_own_colors() {
    let d = ThemesDir::new("tab-active");
    let dir = Some(d.0.as_path());
    d.file("same", "extends = \"nord\"\n[colors]\nwarning = \"#ffcc00\"\n");
    assert_eq!(resolve("same", Background::Dark, dir).unwrap().tab_active, NORD.tab_active);
    d.file("old", "extends = \"dark\"\n[colors]\nbg = \"#000000\"\naccent = \"#ff0000\"\n");
    let old = resolve("old", Background::Dark, dir).unwrap();
    assert_eq!(old.tab_active, Style::new().bg(Color::Rgb(0x4C, 0, 0)), "30% of the accent");
    assert_eq!(old.tab_active, old.tab_active_derived());
    d.file("set", "extends = \"dark\"\n[colors]\nbg = \"#000000\"\n[styles]\ntab_active = { bg = \"blue\" }\n");
    assert_eq!(resolve("set", Background::Dark, dir).unwrap().tab_active, Style::new().bg(Color::Blue));
    // Terminal colors cannot be blended: the extended theme's fill stays.
    d.file("ansi", "extends = \"terminal\"\n[colors]\naccent = \"magenta\"\n");
    assert_eq!(resolve("ansi", Background::Dark, dir).unwrap().tab_active, TERMINAL.tab_active);
    // Without a background of its own the active tab is on the alternate surface.
    let th = Theme { tab_active: Style::new().fg(Color::Red), ..DARK };
    assert_eq!(th.tab_active_bg(), DARK.surface_alt);
}

#[test]
fn mode_badges_read_and_differ_in_truecolor_and_256_colors() {
    for (name, th) in rgb_themes() {
        let badges = [th.mode_normal, th.mode_insert, th.mode_visual, th.mode_command];
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

/// A search match shows on any line of the editor: its own background (the cursor line and
/// the statement tint only patch theirs), unlike the selection's and the cursor line's.
#[test]
fn search_matches_show_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let s = th.search_match;
        assert!(s.bg.is_some_and(|bg| bg != th.bg) && s.fg.is_some(), "{name}: {s:?}");
        assert!(s != th.selection && s.bg != th.cursor_line.bg && s.bg != th.current_stmt.bg, "{name}");
    }
}

/// A match of the grid's search shows on any cell: its own background, apart from the
/// selection, a selected range, the alternate rows, the hover and the copy flash (a copy must
/// not look like a match), its text readable on it. The cursor's match stands apart from the
/// other matches and from the plain cursor, its text readable too.
#[test]
fn grid_matches_show_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let (m, cur) = (th.grid_match, th.grid_match_current);
        assert!(m.bg.is_some() && m.fg.is_some(), "{name}: {m:?}");
        for other in [Some(th.bg), Some(th.surface_alt), th.selection.bg, th.range.bg, th.hover.bg, th.yank_flash.bg] {
            assert_ne!(m.bg, other, "{name}: the match's background");
        }
        assert!(m != th.selection && m != th.range && m != th.hover && m != th.yank_flash, "{name}");
        assert!(cur != m && cur != th.selection && cur != th.selection_unfocused(), "{name}");
        assert!(!cur.add_modifier.is_empty(), "{name}: the cursor's match shows without color");
    }
    for (name, th) in rgb_themes() {
        let m = th.grid_match;
        for other in [th.bg, th.surface_alt, bg(th.selection), bg(th.range), bg(th.hover), bg(th.yank_flash)] {
            let d = delta_e(bg(m), other);
            assert!(d >= 8.0, "{name}: a match and {other:?} are too close ({d:.2})");
        }
        let k = contrast(fg(m), bg(m));
        assert!(k >= 4.5, "{name}: a match's text: {k:.2}");
        let cur = th.grid_match_current;
        let k = contrast(fg(cur), bg(cur));
        assert!(k >= 4.5, "{name}: the cursor's match's text: {k:.2}");
    }
}

/// A plan's hot node and its misestimate marks show without color (a modifier), stand apart
/// from body text and from each other, and read on the background, the surface and the cursor
/// line (the selection of a pane without the focus). On the selection itself they keep only
/// their modifier (the views draw them so).
#[test]
fn plan_marks_show_and_read_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        for (token, s) in [("plan_hot", th.plan_hot), ("plan_misestimate", th.plan_misestimate)] {
            assert!(!s.add_modifier.is_empty(), "{name}: {token} shows without color");
            assert!(s.fg.is_some() && s.fg != Some(th.fg), "{name}: {token} looks like body text");
        }
        assert_ne!(th.plan_hot, th.plan_misestimate, "{name}");
    }
    for (name, th) in rgb_themes() {
        for bg in [th.bg, th.surface, bg(th.cursor_line)] {
            for (token, s) in [("plan_hot", th.plan_hot), ("plan_misestimate", th.plan_misestimate)] {
                let c = contrast(fg(s), bg);
                assert!(c >= 3.0, "{name}: {token} on {bg:?}: {c:.2}");
            }
        }
    }
}

/// A chart's series colors read on the background and on the cursor's column (3:1, marks),
/// stay apart from each other (also in 256 colors) and from the muted "others" series, in
/// every theme; the terminal theme uses six different ANSI colors.
#[test]
fn chart_series_read_and_stay_apart_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let colors = th.chart();
        for (i, a) in colors.iter().enumerate() {
            assert!(*a != th.fg_muted && *a != th.fg && *a != th.bg, "{name}: chart_{} is a text color", i + 1);
            for b in &colors[i + 1..] {
                assert_ne!(a, b, "{name}: two series share a color");
            }
        }
    }
    for (name, th) in rgb_themes() {
        let colors = th.chart();
        for (i, &c) in colors.iter().enumerate() {
            for bg in [th.bg, bg(th.cursor_line)] {
                let k = contrast(c, bg);
                assert!(k >= 3.0, "{name}: chart_{} on {bg:?}: {k:.2}", i + 1);
            }
            assert_ne!(xterm256(c), xterm256(th.bg), "{name}: chart_{} vanishes in 256 colors", i + 1);
            for (j, &d) in colors.iter().enumerate().skip(i + 1) {
                assert_ne!(xterm256(c), xterm256(d), "{name}: chart_{} and chart_{} in 256 colors", i + 1, j + 1);
                let (a, b) = (rgb_of(c), rgb_of(d));
                let dist = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
                assert!(dist >= 40.0, "{name}: chart_{} and chart_{} are too close ({dist:.0})", i + 1, j + 1);
            }
        }
    }
}

/// The statement that runs now stands apart from the one a run would take: its own tint and a
/// bar of another color that reads on every line background it may sit on (3:1, a non-text
/// mark) and differs from the run target's bar also in 256 colors. Its text stays readable.
#[test]
fn the_running_statement_stands_apart_from_the_run_target() {
    for (name, th) in rgb_themes() {
        let (tint, target, cursor_line) = (bg(th.running_stmt), bg(th.current_stmt), bg(th.cursor_line));
        assert!(tint != target && tint != cursor_line && tint != bg(th.selection) && tint != th.bg, "{name}");
        assert!(contrast(th.fg, tint) >= 4.5, "{name}: body text on the tint: {:.2}", contrast(th.fg, tint));
        assert_ne!(xterm256(tint), xterm256(th.bg), "{name}: the tint vanishes in 256 colors");
        for bg in [th.bg, tint, target, cursor_line] {
            let c = contrast(th.running_stmt_bar, bg);
            assert!(c >= 3.0, "{name}: bar on {bg:?}: {c:.2}");
            assert_ne!(xterm256(th.running_stmt_bar), xterm256(bg), "{name}: bar on {bg:?} in 256 colors");
        }
        assert_ne!(xterm256(th.running_stmt_bar), xterm256(th.current_stmt_bar), "{name}: the two bars in 256 colors");
    }
    for (name, th) in BUILTINS.iter().copied() {
        assert_ne!(th.running_stmt_bar, th.current_stmt_bar, "{name}");
    }
}

/// A run's hint is dim but readable on every line background (3:1, as muted text), never
/// looks like the statement's text (another color and a modifier), and its marks (the
/// success, error and warning roles) read there too.
#[test]
fn run_hints_read_and_stay_apart_from_text() {
    for (name, th) in BUILTINS.iter().copied() {
        let s = th.run_hint;
        assert!(s.fg.is_some_and(|c| c != th.fg) && !s.add_modifier.is_empty(), "{name}: {s:?}");
        assert!(s.bg.is_none(), "{name}: the hint keeps the line's background");
    }
    for (name, th) in rgb_themes() {
        for bg in [th.bg, bg(th.cursor_line), bg(th.current_stmt), bg(th.running_stmt)] {
            let c = contrast(fg(th.run_hint), bg);
            assert!(c >= 3.0, "{name}: hint on {bg:?}: {c:.2}");
            assert_ne!(xterm256(fg(th.run_hint)), xterm256(bg), "{name}: hint on {bg:?} in 256 colors");
            for mark in [th.success, th.error, th.warning] {
                let c = contrast(mark, bg);
                assert!(c >= 3.0, "{name}: {mark:?} on {bg:?}: {c:.2}");
            }
        }
    }
}

#[test]
fn executable_comments_stand_apart_from_comments_and_keywords_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let exec = th.syntax(datarig_core::sql::lexer::Tok::ExecComment, false);
        assert_ne!(exec, th.syn_comment, "{name}");
        assert_ne!(exec, th.syn_keyword, "{name}");
        assert_ne!(exec.fg, th.syn_comment.fg, "{name}");
    }
}

/// The size tiers of the explorer's estimates read as a subtle warning: a foreground only
/// (the row's background stays), apart from the dim estimates, from body text and from each
/// other (the two of three levels, and the four of five: yellow, amber, orange, red); in RGB
/// themes they read (3:1, as marks) on the explorer's background and on the selected row's
/// (focused or not), stay apart in 256 colors and are at least 45 apart in RGB. The terminal
/// theme uses its ANSI yellows and reds.
#[test]
fn size_heat_reads_on_the_explorer_rows_in_every_theme() {
    let tiers = |th: &Theme| {
        [
            ("size_mild", th.size_mild),
            ("size_warm", th.size_warm),
            ("size_heated", th.size_heated),
            ("size_hot", th.size_hot),
        ]
    };
    for (name, th) in BUILTINS.iter().copied() {
        let all = tiers(th);
        for (i, (token, s)) in all.iter().enumerate() {
            assert!(s.bg.is_none(), "{name}: {token} keeps the row's background");
            let c = fg(*s);
            assert!(c != th.fg_dim && c != th.fg && c != th.bg, "{name}: {token} looks like text");
            for (other, t) in &all[i + 1..] {
                assert_ne!(c, fg(*t), "{name}: {token} and {other}");
            }
        }
    }
    assert_eq!(tiers(&TERMINAL).map(|(_, s)| fg(s)), [Color::LightYellow, Color::Yellow, Color::LightRed, Color::Red]);
    for (name, th) in rgb_themes() {
        let all = tiers(th);
        for (i, (token, s)) in all.iter().enumerate() {
            let c = fg(*s);
            for bg in [th.bg, bg(th.selection), bg(th.cursor_line), bg(th.selection_unfocused())] {
                let k = contrast(c, bg);
                assert!(k >= 3.0, "{name}: {token} on {bg:?}: {k:.2}");
                assert_ne!(xterm256(c), xterm256(bg), "{name}: {token} on {bg:?} in 256 colors");
            }
            assert_ne!(xterm256(c), xterm256(th.fg_dim), "{name}: {token} and dim in 256 colors");
            for (other, t) in &all[i + 1..] {
                assert_ne!(xterm256(c), xterm256(fg(*t)), "{name}: {token} and {other} in 256 colors");
                let (a, b) = (rgb_of(c), rgb_of(fg(*t)));
                let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
                assert!(d >= 45.0, "{name}: {token} and {other} are too close ({d:.0})");
            }
        }
    }
}

/// The selection of a pane without the focus (the grid's cell, the inspector's entry) stays
/// clearly visible: its own background, close to the selection's and further from the pane's
/// than the faint cursor line it used to be, apart from the background in 256 colors,
/// with body text readable on it; never the focused selection. The terminal theme dims its
/// reversed text.
#[test]
fn the_unfocused_selection_is_visible_and_apart_from_the_focused_one() {
    for (name, th) in BUILTINS.iter().copied() {
        let u = th.selection_unfocused();
        assert_ne!(u, th.selection, "{name}");
        assert_ne!(u, th.cursor_line, "{name}");
    }
    let t = TERMINAL.selection_unfocused();
    assert!(t.add_modifier.contains(Modifier::REVERSED | Modifier::DIM), "{t:?}");
    for (name, th) in rgb_themes() {
        let u = bg(th.selection_unfocused());
        let (sel, faint) = (bg(th.selection), bg(th.cursor_line));
        assert_ne!(u, sel, "{name}");
        let k = contrast(u, th.bg);
        assert!(k > contrast(faint, th.bg), "{name}: {k:.2} not above the cursor line's");
        assert!(k < contrast(sel, th.bg), "{name}: {k:.2} not below the selection's");
        assert_ne!(xterm256(u), xterm256(th.bg), "{name}: vanishes in 256 colors");
        assert!(contrast(th.fg, u) >= 4.5, "{name}: body text on it: {:.2}", contrast(th.fg, u));
    }
}

/// A profile's name in its environment's color reads as text (4.5:1) on the background and
/// both surfaces of every theme, for every profile color and without one (body text); dark
/// themes keep the color as it is. Not connected, the same color muted: still 3:1 there, yet
/// clearly fainter (its contrast at least 1.3 times lower). Without
/// RGB backgrounds the color stays and muting is the dim modifier.
#[test]
fn environment_colored_names_read_and_muted_ones_stay_readable() {
    let envs: Vec<Option<Color>> = std::iter::once(None).chain(PROFILE_COLORS.iter().copied().map(Some)).collect();
    for (name, th) in rgb_themes() {
        for &env in &envs {
            let c = th.name_color(env);
            let muted = fg(th.name_style(env, false));
            assert_eq!(th.name_style(env, true), Style::new().fg(c).add_modifier(Modifier::BOLD), "{name}");
            for back in [th.bg, th.surface, th.surface_alt] {
                assert!(contrast(c, back) >= 4.5, "{name}: {env:?} on {back:?}: {:.2}", contrast(c, back));
                assert!(
                    contrast(muted, back) >= 3.0,
                    "{name}: muted {env:?} on {back:?}: {:.2}",
                    contrast(muted, back)
                );
            }
            let (on, off) = (contrast(c, th.bg), contrast(muted, th.bg));
            assert!(on >= off * 1.3, "{name}: {env:?}: {on:.2} vs muted {off:.2}");
        }
        assert_eq!(th.name_color(None), th.fg, "{name}");
    }
    for &c in &PROFILE_COLORS {
        assert_eq!(DARK.name_color(Some(c)), c, "dark keeps the profile colors");
    }
    let th = &TERMINAL;
    assert_eq!(
        th.name_style(Some(PROFILE_COLORS[0]), true),
        Style::new().fg(PROFILE_COLORS[0]).add_modifier(Modifier::BOLD)
    );
    assert_eq!(th.name_style(None, false), Style::new().fg(Color::Reset).add_modifier(Modifier::DIM));
}

/// An environment's badge chip reads (4.5:1) in every theme, for every profile color (kept as
/// it is) and for a badge without a color (the theme's muted text color, as `App::badge_color`
/// picks it, moved a little where it is of middle lightness).
#[test]
fn environment_badge_chips_read_in_every_theme() {
    for &c in &PROFILE_COLORS {
        assert_eq!(chip(c).bg, Some(c), "a profile color stays");
    }
    for (name, th) in rgb_themes() {
        for c in PROFILE_COLORS.iter().copied().chain([th.fg_muted]) {
            let s = chip(c);
            assert!(s.add_modifier.contains(Modifier::BOLD));
            let k = contrast(fg(s), bg(s));
            assert!(k >= 4.5, "{name}: chip of {c:?}: {k:.2}");
            let (a, b) = (rgb_of(c), rgb_of(bg(s)));
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
            assert!(d < 60.0, "{name}: chip of {c:?} moved too far ({d:.0})");
        }
    }
    assert_eq!(
        chip(Color::Gray),
        Style::new().fg(Color::Rgb(0x16, 0x16, 0x1E)).bg(Color::Gray).add_modifier(Modifier::BOLD)
    );
}

/// An environment's rules in words take a color each (`app::environments::RuleTone`): the
/// read-only mark's blue (also the lock of read-only rules), the warning color for asking
/// before every write, the error color for holding locks while paging, dim for the standard
/// rules (the dim text as everywhere). The four stand apart in every theme (also in 256
/// colors), and the three colored ones read (3:1) on the background and both surfaces (the
/// explorer, the lists and forms); the read-only mark reads as text (4.5:1) there.
#[test]
fn environment_rule_tones_read_and_stand_apart_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let tones = [fg(th.read_only_mark), th.warning, th.error, th.fg_dim];
        for (i, a) in tones.iter().enumerate() {
            for b in &tones[i + 1..] {
                assert_ne!(a, b, "{name}");
            }
        }
    }
    for (name, th) in rgb_themes() {
        let tones = [fg(th.read_only_mark), th.warning, th.error, th.fg_dim];
        for back in [th.bg, th.surface, th.surface_alt] {
            for c in &tones[..3] {
                let c = *c;
                assert!(contrast(c, back) >= 3.0, "{name}: {c:?} on {back:?}: {:.2}", contrast(c, back));
            }
            let k = contrast(fg(th.read_only_mark), back);
            assert!(k >= 4.5, "{name}: the read-only mark on {back:?}: {k:.2}");
        }
        let mapped = tones.map(xterm256);
        for i in 0..4 {
            for j in i + 1..4 {
                assert_ne!(mapped[i], mapped[j], "{name}: {:?} and {:?} in 256 colors", tones[i], tones[j]);
            }
        }
    }
}

/// The CIE76 color difference of two RGB colors (sRGB to XYZ under D65 to CIELAB).
fn delta_e(a: Color, b: Color) -> f64 {
    let lab = |c: Color| {
        let (r, g, b) = rgb_of(c);
        let lin = |v: f64| {
            let v = v / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(r), lin(g), lin(b));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
    };
    let (p, q) = (lab(a), lab(b));
    ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt()
}

/// What is under the pointer has a background of its own and is never underlined: on the
/// terminal theme the bright black with the terminal's text color; on the others a background
/// that stands apart (5 ΔE or more) from the background, both surfaces (the grid's alternate
/// rows are on the alternate surface), the cursor line, the selection (focused or not) and a
/// selected range, further from the background than nothing and nearer to it than the
/// selection, that does not vanish in 256 colors, and on which body text, muted text and the
/// accent read.
#[test]
fn hover_is_a_background_apart_from_the_surfaces_and_the_selections_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        assert!(th.hover.bg.is_some(), "{name}");
        assert!(!th.hover.add_modifier.contains(Modifier::UNDERLINED), "{name}");
    }
    let t = &TERMINAL;
    assert_eq!((t.hover.bg, t.hover.fg), (Some(Color::DarkGray), Some(Color::Reset)));
    for other in [t.selection, t.selection_unfocused(), t.range, t.cursor_line] {
        assert_ne!(t.hover, other);
    }
    assert!(Some(t.bg) != t.hover.bg && Some(t.surface_alt) != t.hover.bg, "apart from the rows");
    for (name, th) in rgb_themes() {
        let h = bg(th.hover);
        for other in [
            th.bg,
            th.surface,
            th.surface_alt,
            bg(th.cursor_line),
            bg(th.selection),
            bg(th.selection_unfocused()),
            bg(th.range),
        ] {
            let d = delta_e(h, other);
            assert!(d >= 5.0, "{name}: the hover and {other:?} are too close ({d:.2})");
        }
        assert_ne!(xterm256(h), xterm256(th.bg), "{name}: vanishes in 256 colors");
        let (k, sel) = (contrast(h, th.bg), contrast(bg(th.selection), th.bg));
        assert!(
            k > 1.0 && k < sel,
            "{name}: the hover ({k:.3}) is not between the background and the selection ({sel:.3})"
        );
        for (c, min) in [(th.fg, 4.5), (th.fg_muted, 3.0), (th.accent, 3.0)] {
            let k = contrast(c, h);
            assert!(k >= min, "{name}: {c:?} on the hover: {k:.2}");
        }
    }
}

/// Copied cells flash on a background of their own with a text color of their own (the cell's
/// NULL or muted text would not read on it): never underlined; on the terminal theme black on
/// the yellow, apart from the reversed selection (the cursor's cell flashes too), the range and
/// the hover; on the others a background that stands apart (10 ΔE or more) from the
/// background, both surfaces, the cursor line, the selection (focused or not), a selected range
/// and the hover, also in 256 colors, with its text reading on it at 4.5:1.
#[test]
fn copied_cells_flash_apart_from_every_grid_background_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        let s = th.yank_flash;
        assert!(s.bg.is_some() && s.fg.is_some(), "{name}: {s:?}");
        assert!(!s.add_modifier.contains(Modifier::UNDERLINED), "{name}");
    }
    let t = &TERMINAL;
    assert_eq!((t.yank_flash.fg, t.yank_flash.bg), (Some(Color::Black), Some(Color::Yellow)));
    for other in [t.selection, t.selection_unfocused(), t.range, t.cursor_line, t.hover] {
        assert_ne!(t.yank_flash, other);
        assert_ne!(t.yank_flash.bg, other.bg, "{other:?}");
    }
    for (name, th) in rgb_themes() {
        let f = bg(th.yank_flash);
        for other in [
            th.bg,
            th.surface,
            th.surface_alt,
            bg(th.cursor_line),
            bg(th.selection),
            bg(th.selection_unfocused()),
            bg(th.range),
            bg(th.hover),
        ] {
            let d = delta_e(f, other);
            assert!(d >= 10.0, "{name}: the flash and {other:?} are too close ({d:.2})");
            assert_ne!(xterm256(f), xterm256(other), "{name}: the flash and {other:?} in 256 colors");
        }
        let k = contrast(fg(th.yank_flash), f);
        assert!(k >= 4.5, "{name}: text on the flash: {k:.2}");
    }
}

/// The active tab is on a fill of its own that stands out clearly and is never underlined: on
/// the terminal theme black on the cyan (apart from the reversed selection a dragged tab takes,
/// the hover's bright black, the range, the cursor line and the copied cells' yellow); on the
/// others a background with no text color of its own (the tab's colors stay), apart (8 ΔE or
/// more) from the background, both surfaces, the cursor line, the selection (focused or not), a
/// selected range, the hover and the flash, also in 256 colors, further from the background
/// than the hover; the tab's name reads on it as text (4.5:1) in body text and every
/// environment color, its number, marks and `×` (also the error color it takes under the
/// pointer) as marks (3:1), as [`Theme::readable_on`] makes them, which keeps a color that
/// already reads.
#[test]
fn the_active_tab_stands_apart_from_every_other_background_in_every_theme() {
    for (name, th) in BUILTINS.iter().copied() {
        assert!(th.tab_active.bg.is_some(), "{name}");
        assert!(!th.tab_active.add_modifier.intersects(Modifier::UNDERLINED | Modifier::REVERSED), "{name}");
        assert_eq!(th.tab_active_bg(), bg(th.tab_active), "{name}");
    }
    let t = &TERMINAL;
    assert_eq!(t.tab_active, Style::new().fg(Color::Black).bg(Color::Cyan));
    assert_eq!(t.tab_active.bg, Some(t.accent), "the accent's cyan");
    for other in [t.selection, t.selection_unfocused(), t.range, t.cursor_line, t.hover, t.yank_flash] {
        assert_ne!(t.tab_active, other);
        assert_ne!(t.tab_active.bg, other.bg, "{other:?}");
    }
    assert_ne!(t.tab_active.patch(t.selection), t.tab_active, "a dragged active tab looks lifted");
    for (name, th) in rgb_themes() {
        assert_eq!(th.tab_active.fg, None, "{name}: the tab's own colors stay");
        let f = bg(th.tab_active);
        for other in [
            th.bg,
            th.surface,
            th.surface_alt,
            bg(th.cursor_line),
            bg(th.selection),
            bg(th.selection_unfocused()),
            bg(th.range),
            bg(th.hover),
            bg(th.yank_flash),
        ] {
            let d = delta_e(f, other);
            assert!(d >= 8.0, "{name}: the active tab and {other:?} are too close ({d:.2})");
        }
        for other in [th.bg, th.surface_alt, bg(th.hover), bg(th.selection)] {
            assert_ne!(xterm256(f), xterm256(other), "{name}: the active tab and {other:?} in 256 colors");
        }
        let (k, hover) = (contrast(f, th.bg), contrast(bg(th.hover), th.bg));
        assert!(k > hover, "{name}: the active tab ({k:.3}) stands out less than the hover ({hover:.3})");
        assert!(contrast(th.fg, f) >= 4.5, "{name}: body text on it: {:.2}", contrast(th.fg, f));
        for env in std::iter::once(None).chain(PROFILE_COLORS.iter().copied().map(Some)) {
            let c = Theme::readable_on(th.name_color(env), f, 4.5);
            assert!(contrast(c, f) >= 4.5, "{name}: {env:?} as the name: {:.2}", contrast(c, f));
        }
        let marks = [th.fg_muted, th.accent, th.accent_warm, th.warning, th.error, fg(th.read_only_mark)];
        for c in PROFILE_COLORS.iter().copied().chain(marks) {
            let m = Theme::readable_on(c, f, 3.0);
            assert!(contrast(m, f) >= 3.0, "{name}: {c:?} as a mark: {:.2}", contrast(m, f));
            if contrast(c, f) >= 3.0 {
                assert_eq!(m, c, "{name}: a color that reads stays");
            }
        }
    }
    assert_eq!(Theme::readable_on(Color::Red, bg(DARK.tab_active), 3.0), Color::Red, "a terminal color stays");
}
