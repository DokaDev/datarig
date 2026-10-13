//! Environment colors on screen: a profile's name takes its environment's color in the
//! explorer (its own row and the rows under an environment), the tab list and the quick
//! switcher, bold while connected and muted while not (its icon and state mark keep the
//! profile's color); a tab's title takes it in the tab bar; and an environment's badge is the
//! same readable chip wherever it is drawn, the explorer's rows included.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::DbEvent;
use datarig_core::i18n::Lang;
use datarig_core::policy::{Environment, Policy};
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::{Focus, NodeState};
use datarig_tui::theme::{self, Theme};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::sync::Arc;

const W: u16 = 120;
const H: u16 = 40;

/// Red, as the profile palette has it (`prod`'s color).
fn red() -> Color {
    theme::PROFILE_COLORS[0]
}

/// `local-pg` names `prod` (red, `PROD`); `分析-replica` names `dev` (green, no badge); `v6`
/// none.
fn config() -> Config {
    let mut cfg = sample_config(None);
    cfg.connections[0].policy = Some("prod".into());
    cfg.connections[1].policy = Some("dev".into());
    let mut prod = Environment::new("prod", Policy::default());
    (prod.color, prod.badge) = (Some("red".into()), Some("PROD".into()));
    let mut dev = Environment::new("dev", Policy::default());
    dev.color = Some("green".into());
    cfg.policies.environments_mut().extend([prod, dev]);
    cfg
}

fn with_theme(h: &mut Harness, th: &Theme) {
    h.app.theme = Arc::new(th.clone());
}

/// Where `text` starts in `area` of `buf` (one symbol per cell; the first match row by row).
fn find(buf: &Buffer, area: Rect, text: &str) -> Option<(u16, u16)> {
    let want: Vec<String> = text.chars().map(String::from).collect();
    for y in area.y..area.y + area.height {
        for x in area.x..(area.x + area.width).saturating_sub(want.len() as u16 - 1) {
            if want.iter().enumerate().all(|(i, s)| buf[(x + i as u16, y)].symbol() == s) {
                return Some((x, y));
            }
        }
    }
    None
}

/// The style `text` is drawn in (its first cell) in `area`, as drawn now.
fn style_of(h: &mut Harness, area: impl Fn(&Harness) -> Rect, text: &str) -> Style {
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    let (x, y) = find(buf, area(h), text).unwrap_or_else(|| panic!("no {text:?} on\n{}", h.screen(W, H)));
    buf[(x, y)].style()
}

fn explorer(h: &Harness) -> Rect {
    h.app.layout.tree
}

/// The first cell of `text` keeps `fg` and the modifiers of `want` (the background is the row's).
fn assert_text(s: Style, want: Style, what: &str) {
    assert_eq!(s.fg, want.fg, "{what}: {s:?}");
    assert_eq!(s.add_modifier & (Modifier::BOLD | Modifier::DIM), want.add_modifier, "{what}: {s:?}");
}

/// The badge of an environment in the explorer's Environments section is its chip, unfocused,
/// focused and under the cursor, in every theme: its own background and readable text, not the
/// row's background under the chip's dark text.
#[test]
fn the_explorer_draws_an_environments_badge_as_its_chip() {
    for (name, th) in theme::BUILTINS.iter().copied() {
        let mut h = Harness::with_config(&config(), Lang::En);
        with_theme(&mut h, th);
        let chip = {
            let _scope = theme::scope(Arc::new(th.clone()));
            theme::chip(red())
        };
        let check = |h: &mut Harness, what: &str| {
            let t = h.draw(W, H);
            let buf = t.backend().buffer();
            let (x, y) = find(buf, h.app.layout.tree, " PROD ").expect("the badge in the explorer");
            for i in 0..6 {
                let c = &buf[(x + i, y)];
                assert_eq!((c.fg, c.bg), (chip.fg.unwrap(), chip.bg.unwrap()), "{name}: {what}: cell {i}");
            }
        };
        check(&mut h, "unfocused");
        let id = h.app.environments().iter().find(|e| e.name == "prod").unwrap().id;
        h.app.focus = Focus::Tree;
        h.app.explorer.select_kind(RowKind::Environment(id));
        check(&mut h, "under the focused cursor");
        h.app.focus = Focus::Editor;
        check(&mut h, "under the cursor, unfocused");
    }
}

/// The chip of a badge without a color reads in the terminal theme too: light gray under the
/// chip's dark text (the theme's muted color is the terminal's dark gray).
#[test]
fn a_badge_without_a_color_is_a_light_chip_in_the_terminal_theme() {
    let mut cfg = config();
    cfg.policies.environments_mut()[0].color = None;
    let mut h = Harness::with_config(&cfg, Lang::En);
    with_theme(&mut h, &theme::TERMINAL);
    let s = style_of(&mut h, explorer, " PROD ");
    assert_eq!((s.fg, s.bg), (Some(Color::Rgb(0x16, 0x16, 0x1E)), Some(Color::Gray)));
}

/// A profile's name in the explorer: its environment's color (body text without one), muted
/// while not connected, bold once connected; its state mark and icon keep the profile's color.
/// The same under the environment that names it.
#[test]
fn explorer_names_take_their_environments_color_muted_until_connected() {
    for (name, th) in [("dark", &theme::DARK), ("light", &theme::LIGHT), ("terminal", &theme::TERMINAL)] {
        let mut h = Harness::with_config(&config(), Lang::En);
        with_theme(&mut h, th);
        let green = theme::PROFILE_COLORS[3];
        let off = |env: Option<Color>| th.name_style(env, false);
        assert_text(style_of(&mut h, explorer, "local-pg"), off(Some(red())), name);
        assert_text(style_of(&mut h, explorer, "-replica"), off(Some(green)), name);
        assert_text(style_of(&mut h, explorer, "v6"), off(None), name);
        // Not connected, the state mark keeps the profile's color (`local-pg` is connecting).
        let v6 = h.app.profiles[2].clone();
        let mark = style_of(&mut h, explorer, &format!("○   {}", v6.name));
        assert_eq!(mark.fg, Some(theme::profile_color(v6.display_color())), "{name}");
        let local = h.app.profiles[0].clone();
        h.db(DbEvent::Connected);
        assert_eq!(h.app.conns.state(local.id), NodeState::Connected);
        assert_text(style_of(&mut h, explorer, "local-pg"), th.name_style(Some(red()), true), name);
        let mark = style_of(&mut h, explorer, &format!("●   {}", local.name));
        assert_eq!(mark.fg, Some(theme::profile_color(local.display_color())), "{name}");
        // Under its environment, opened.
        let id = h.app.environments().iter().find(|e| e.name == "prod").unwrap().id;
        h.app.focus = Focus::Tree;
        h.app.explorer.select_kind(RowKind::Environment(id));
        h.key(KeyCode::Right);
        let rows = h.app.explorer_rows();
        assert!(rows.iter().any(|r| matches!(r.kind, RowKind::EnvironmentUser(..))), "{:?}", h.rows());
        let t = h.draw(W, H);
        let buf = t.backend().buffer();
        let tree = h.app.layout.tree;
        let under: Vec<(u16, u16)> = (tree.y..tree.y + tree.height)
            .filter_map(|y| find(buf, Rect::new(tree.x, y, tree.width, 1), "local-pg"))
            .collect();
        assert_eq!(under.len(), 2, "its row and the one under prod");
        for (x, y) in under {
            assert_text(buf[(x, y)].style(), th.name_style(Some(red()), true), name);
        }
    }
}

/// The tab bar's titles: the color of the tab's connection's environment, bold on the active
/// tab, muted on another; without an environment the body text (muted on another tab) as
/// before. The badge chip stays.
#[test]
fn tab_titles_take_their_connections_environment_color() {
    let mut h = Harness::with_config(&config(), Lang::En);
    let th = theme::DARK;
    let bar = |h: &Harness| h.app.layout.tab_bar;
    assert_text(style_of(&mut h, bar, "console 1"), th.name_style(Some(red()), true), "active");
    let s = style_of(&mut h, bar, " PROD ");
    assert_eq!(s.bg, Some(red()), "the chip stays");
    // A second tab, made active: the first one's title is muted now.
    h.ctrl('t');
    assert_eq!(h.app.tabs.len(), 2);
    let first = style_of(&mut h, bar, "console 1");
    assert_eq!(first.fg, th.name_style(Some(red()), false).fg);
    assert!(!first.add_modifier.contains(Modifier::BOLD));
    assert_text(style_of(&mut h, bar, "console 2"), th.name_style(Some(red()), true), "active");
    // Without a connection: the body text, as before.
    h.app.tab_mut().profile = None;
    let second = style_of(&mut h, bar, "console 2");
    assert_eq!(second.fg, Some(th.fg), "no environment: as before");
}

/// The quick switcher and the tab list draw a profile's name as the explorer does.
#[test]
fn the_quick_switcher_and_the_tab_list_color_names_by_environment() {
    let mut h = Harness::with_config(&config(), Lang::En);
    let th = theme::DARK;
    h.db(DbEvent::Connected);
    h.ctrl('o');
    assert!(h.app.overlays.quick().is_some());
    h.draw(W, H);
    let quick = |h: &Harness| h.app.overlays.quick().map(|q| q.list).unwrap_or_default();
    assert_text(style_of(&mut h, quick, "local-pg"), th.name_style(Some(red()), true), "quick: connected");
    assert_text(style_of(&mut h, quick, "v6"), th.name_style(None, false), "quick: no environment");
    h.key(KeyCode::Esc);
    h.app.focus = Focus::Tree;
    h.keys(" tt");
    assert!(h.app.overlays.tab_list().is_some(), "{}", h.screen(W, H));
    let list = |h: &Harness| h.app.overlays.tab_list().map(|l| l.rows[0].0).unwrap_or_default();
    assert_text(style_of(&mut h, list, "local-pg"), th.name_style(Some(red()), true), "tab list");
}
