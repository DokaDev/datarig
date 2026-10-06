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
        key_pk: _,
        key_fk: _,
        key_uq: _,
        mode_normal: _,
        mode_insert: _,
        mode_visual: _,
        mode_command: _,
        mode_fg: _,
        search_match: _,
        match_paren: _,
        read_only_mark: _,
        danger_mark: _,
        plan_hot: _,
        plan_misestimate: _,
        dim: _,
    } = DARK;
    // Every field above but `dim`.
    assert_eq!(COLOR_TOKENS.len() + STYLE_TOKENS.len(), 43);
    let mut th = DARK;
    for (i, name) in COLOR_TOKENS.iter().enumerate() {
        *color_token(&mut th, name).unwrap() = Color::Indexed(i as u8);
    }
    for (i, name) in STYLE_TOKENS.iter().enumerate() {
        *style_token(&mut th, name).unwrap() = Style::new().bg(Color::Indexed(100 + i as u8));
    }
    assert_eq!((th.bg, th.mode_fg), (Color::Indexed(0), Color::Indexed(22)));
    assert_eq!(th.plan_misestimate, Style::new().bg(Color::Indexed(119)));
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
