//! The explorer's sections look alike: "Connections" (the folders and profiles, a head of its
//! own that opens and closes and is kept in the workspace file), "Tunnels", "Environments" and
//! "Saved queries", each head an icon (with icons on), its name and how many it has. A tunnel
//! preset's icon is in the color of its connection's state, an environment's chip in its color;
//! circles stand for a profile's connection only (and for those marks with icons off). The
//! read-only rules are a lock (`RO` with icons off) in the read-only mark's blue on a profile's
//! row and its tabs, and an environment's rules are colored by what they do.

mod common;

use common::*;
use datarig_core::config::{Config, IconsSetting};
use datarig_core::i18n::Lang;
use datarig_core::policy::{Confirm, Environment, Policy};
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::profile::tunnel::TunnelPreset;
use datarig_core::secret::MemoryStore;
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::{Focus, Startup};
use datarig_tui::icons;
use datarig_tui::theme::{self, DARK};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use std::sync::Arc;

/// The sample profiles: `local-pg` in `prod` (read-only, asks before every write, holds locks
/// while paging, red, `PROD`) through the preset `office`; `v6` in `dev` (green); a saved query.
fn config() -> Config {
    let mut cfg = sample_config(None);
    let bastion = SshSettings {
        enabled: true,
        host: "bastion.example.com".into(),
        user: "ec2-user".into(),
        auth: SshAuth::Password,
        ..SshSettings::default()
    };
    cfg.tunnels = vec![TunnelPreset::new("office", bastion)];
    cfg.connections[0].tunnel = Some("office".into());
    cfg.connections[0].policy = Some("prod".into());
    cfg.connections[2].policy = Some("dev".into());
    let rules = Policy {
        read_only: true,
        confirm: Confirm::Writes,
        paging: datarig_core::driver::PagingMode::Hold,
        ..Policy::default()
    };
    let mut prod = Environment::new("prod", rules);
    (prod.color, prod.badge) = (Some("red".into()), Some("PROD".into()));
    let mut dev = Environment::new("dev", Policy::default());
    dev.color = Some("green".into());
    cfg.policies.environments_mut().extend([prod, dev]);
    cfg
}

fn launched(icons: bool) -> Harness {
    let mut h = Harness::launched(&config(), Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.app.icons = if icons { IconsSetting::On } else { IconsSetting::Off };
    h
}

/// Where `text` starts in `area` (one symbol per cell).
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

/// The explorer's line of the row of `kind`, whole.
fn line(h: &mut Harness, kind: RowKind) -> String {
    h.app.focus = Focus::Tree;
    h.app.explorer.select_kind(kind);
    h.explorer_line()
}

#[test]
fn every_section_head_has_its_icon_name_and_count() {
    let mut h = launched(true);
    for (kind, want) in [
        (RowKind::ConnectionsHeader, format!("{} Connections 3", icons::CONNECTIONS)),
        (RowKind::TunnelsHeader, format!("{} Tunnels 1", icons::TUNNELS)),
        (RowKind::EnvironmentsHeader, format!("{} Environments 2", icons::ENVIRONMENT)),
    ] {
        let l = line(&mut h, kind);
        assert!(l.ends_with(&want), "{l:?}");
    }
    let mut h = launched(false);
    for (kind, want) in [
        (RowKind::ConnectionsHeader, "▾ Connections 3"),
        (RowKind::TunnelsHeader, "▾ Tunnels 1"),
        (RowKind::EnvironmentsHeader, "▾ Environments 2"),
    ] {
        assert_eq!(line(&mut h, kind).trim(), want, "icons off: the name alone");
    }
    // The heads look alike: the accent, bold, the count dim.
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    for name in ["Connections", "Tunnels", "Environments"] {
        let (x, y) = find(buf, h.app.layout.tree, name).unwrap();
        assert_eq!(buf[(x, y)].fg, DARK.accent, "{name}");
        assert!(buf[(x, y)].modifier.contains(ratatui::style::Modifier::BOLD), "{name}");
    }
}

/// The Connections head opens and closes like the other heads (its profiles and folders under
/// it), a reveal opens it again, and the workspace file keeps it closed.
#[test]
fn the_connections_section_opens_closes_and_is_kept() {
    let mut h = launched(false);
    assert_eq!(h.rows()[..5], ["+", "[connections]", "local-pg", "v6", "分析-replica"]);
    h.app.focus = Focus::Tree;
    h.app.explorer.select_kind(RowKind::ConnectionsHeader);
    h.key(KeyCode::Char('h'));
    assert!(!h.app.connections_expanded);
    assert_eq!(h.rows()[..3], ["+", "[connections]", "[tunnels]"], "closed: the profiles go");
    assert!(h.app.workspace_state().explorer.connections_collapsed, "kept in the workspace file");
    h.key(KeyCode::Enter);
    assert!(h.app.connections_expanded, "Enter toggles it");
    h.key(KeyCode::Char('h'));
    // A filter shows what matches inside a closed section.
    h.keys("/");
    h.type_text("v6");
    assert_eq!(h.rows()[..3], ["+", "[connections]", "v6"]);
    h.key(KeyCode::Esc);
    // `Enter` on a profile under an environment reveals it: the section opens.
    let env = h.app.environments().iter().find(|e| e.name == "dev").unwrap().id;
    h.app.explorer.select_kind(RowKind::Environment(env));
    h.key(KeyCode::Char('l'));
    let v6 = h.app.profiles[2].id;
    h.app.explorer.select_kind(RowKind::EnvironmentUser(env, v6));
    h.key(KeyCode::Enter);
    assert!(h.app.connections_expanded);
    assert_eq!(h.app.selected_profile(), Some(v6));
    assert!(!h.app.workspace_state().explorer.connections_collapsed);
}

/// A tunnel preset's icon takes its state's color (muted while closed) where its circle was;
/// with icons off the circle stays. An environment's chip is in its color; with icons off a
/// dot. A profile through a tunnel carries the same tunnel glyph (`SSH` with icons off).
#[test]
fn presets_and_environments_have_icons_in_their_colors() {
    let mut h = launched(true);
    let office = h.app.presets[0].id;
    let l = line(&mut h, RowKind::Tunnel(office));
    assert!(l.contains(&format!("{} office", icons::TUNNEL)) && !l.contains('○'), "{l:?}");
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let tree = h.app.layout.tree;
    let (x, y) = find(buf, tree, &format!("{} office", icons::TUNNEL)).unwrap();
    assert_eq!(buf[(x, y)].fg, DARK.fg_muted, "closed: muted");
    let (x, y) = find(buf, tree, &format!("{} prod", icons::ENVIRONMENT_CHIP)).unwrap();
    assert_eq!(buf[(x, y)].fg, theme::PROFILE_COLORS[0], "the chip in the environment's color");
    assert!(find(buf, tree, &format!("local-pg {}", icons::TUNNEL)).is_some(), "the tunnel mark of a profile");
    assert!(!(tree.y..tree.y + tree.height).any(|y| {
        let row = row_text(buf, y);
        row.contains("prod") && row.contains('●')
    }));
    let mut h = launched(false);
    let office = h.app.presets[0].id;
    assert!(line(&mut h, RowKind::Tunnel(office)).contains("○ office"));
    let prod = h.app.environments()[0].id;
    assert!(line(&mut h, RowKind::Environment(prod)).contains("● prod"));
    h.explore("local-pg");
    assert!(h.explorer_line().contains("local-pg SSH"));
}

/// Read-only rules: the lock after the profile's name and on its tab (`RO` with icons off), in
/// the read-only mark's blue; never for a profile whose rules are not read-only, whatever its
/// environment is called.
#[test]
fn read_only_rules_show_a_lock_in_blue() {
    for icons_on in [true, false] {
        let mut h = launched(icons_on);
        let mark = if icons_on { icons::READ_ONLY } else { "RO" };
        h.explore("local-pg");
        assert!(h.explorer_line().ends_with(&format!(" {mark}")), "{}", h.explorer_line());
        h.explore("v6");
        assert!(!h.explorer_line().contains(mark));
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let (x, y) = find(buf, h.app.layout.tree, &format!("local-pg {} {mark}", icons::tunnel(icons_on))).unwrap();
        let at = x + format!("local-pg {} ", icons::tunnel(icons_on)).chars().count() as u16;
        assert_eq!(buf[(at, y)].fg, DARK.read_only_mark.fg.unwrap(), "icons {icons_on}");
    }
    // Renamed, the environment keeps its rules and the lock; another one named `prod` without
    // read-only has none.
    let mut cfg = config();
    cfg.policies.environments_mut()[0].name = "live".into();
    cfg.connections[0].policy = Some("live".into());
    cfg.policies.environments_mut()[1].name = "prod".into();
    cfg.connections[2].policy = Some("prod".into());
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.explore("local-pg");
    assert!(h.explorer_line().ends_with(" RO"), "{}", h.explorer_line());
    h.explore("v6");
    assert!(!h.explorer_line().contains(" RO"), "a name decides nothing");
}

/// An environment's rules in words, colored by what they do, the same in the explorer, the
/// environment list and the profile form: read-only in blue (after the lock with icons on),
/// asking before every write in the warning color, holding locks while paging in the error
/// color, the standard rules dim.
#[test]
fn environment_rules_are_colored_by_what_they_do() {
    let mut h = launched(true);
    // A wide explorer: room for the whole rules.
    h.app.explorer.width = Some(120);
    let t = h.draw(240, 45);
    let buf2 = t.backend().buffer();
    let tree2 = h.app.layout.tree;
    for (text, color) in [
        (format!("{} read-only", icons::READ_ONLY), DARK.read_only_mark.fg.unwrap()),
        ("asks before every write".to_string(), DARK.warning),
        ("holds locks while paging".to_string(), DARK.error),
        ("standard rules".to_string(), DARK.fg_dim),
    ] {
        let (x, y) = find(buf2, tree2, &text).unwrap_or_else(|| panic!("{text:?}"));
        assert_eq!(buf2[(x, y)].fg, color, "{text}");
    }
    // The environment list of a profile's form, the same tones.
    h.explore("v6");
    h.keys("e");
    h.ctrl('n');
    h.ctrl('n');
    while h.form().focus != datarig_tui::app::profiles::Field::Environment {
        h.key(KeyCode::Tab);
    }
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(datarig_tui::app::overlay::OverlayKind::Picker), "{}", h.screen(200, 45));
    let t = h.draw(200, 45);
    let buf = t.backend().buffer();
    // The list is on top (the form behind it is dimmed): its row of `prod` (its hint cut at
    // the list's width).
    let (x, y) = (0..45)
        .find_map(|y| {
            let at = find(buf, Rect::new(0, y, 200, 1), "asks before")?;
            (buf[at].fg == DARK.warning).then_some(at)
        })
        .expect("in the list, in the warning color");
    let (rx, _) = find(buf, Rect::new(0, y, 200, 1), "read-only").expect("on the same row");
    assert_eq!(buf[(rx, y)].fg, DARK.read_only_mark.fg.unwrap());
    assert!(rx < x);
}

/// The explorer's sections at 80x24 and 120x40, icons on and off.
#[test]
fn explorer_sections_snapshots() {
    for icons_on in [false, true] {
        for (w, hh) in [(80, 24), (120, 40)] {
            let mut h = launched(icons_on);
            let tag = if icons_on { "icons" } else { "text" };
            assert_screen!(format!("explorer_sections_{tag}_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        }
    }
}
