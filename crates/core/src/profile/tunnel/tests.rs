use super::*;

fn settings() -> SshSettings {
    SshSettings {
        enabled: true,
        host: "bastion".into(),
        user: "ec2-user".into(),
        key_file: Some("~/.ssh/office.pem".into()),
        ..SshSettings::default()
    }
}

#[test]
fn ids_and_accounts() {
    let id = TunnelId::new();
    assert_eq!(TunnelId::parse(&id.to_string()), Some(id));
    assert_eq!(TunnelId::parse("00000000-0000-0000-0000-000000000000"), None, "nil is no id");
    assert_eq!(TunnelId::parse("office"), None);
    assert_eq!(id.account(), format!("tunnel:{id}"));
    assert_ne!(id.account(), crate::profile::ProfileId::new().account());
    let p = TunnelPreset::new("office", SshSettings { enabled: false, ..settings() });
    assert!(p.settings.enabled, "a preset is always on");
    assert_eq!(p.origin, None);
}

#[test]
fn names() {
    assert_eq!(name_problem("office"), None);
    assert_eq!(name_problem("prod bastion"), None, "inner blanks are fine");
    assert_eq!(name_problem("\u{c0ac}\u{bb34}\u{c2e4}"), None, "any language");
    assert_eq!(name_problem(""), Some(NameProblem::Empty));
    assert_eq!(name_problem(" office"), Some(NameProblem::Spaces));
    assert_eq!(name_problem("office "), Some(NameProblem::Spaces));
    assert_eq!(name_problem("of\tfice"), Some(NameProblem::Control));
    assert_eq!(name_problem(&"x".repeat(NAME_MAX)), None);
    assert_eq!(name_problem(&"x".repeat(NAME_MAX + 1)), Some(NameProblem::TooLong));
    assert!(same_name("Office", "office"));
    assert!(!same_name("office", "office2"));
}

#[test]
fn a_profile_routes_directly_through_its_own_tunnel_or_through_a_preset() {
    let presets = vec![TunnelPreset::new("office", settings())];
    let mut c = ConnectionConfig::default();
    assert_eq!(route(&c, &presets), Ok(Route::Direct));
    assert_eq!(route(&c, &presets).unwrap().settings(), None);
    // Its own table, turned off: still direct.
    c.ssh = Some(SshSettings { enabled: false, ..settings() });
    assert_eq!(route(&c, &presets), Ok(Route::Direct));
    c.ssh = Some(settings());
    assert_eq!(route(&c, &presets), Ok(Route::Inline(c.ssh.as_ref().unwrap())));
    // A preset and its own table on: an error, never one of them picked.
    c.tunnel = Some("office".into());
    assert_eq!(route(&c, &presets), Err(RouteError::Both { tunnel: "office".into() }));
    // A preset with its own table turned off: the preset.
    c.ssh = Some(SshSettings { enabled: false, ..settings() });
    assert_eq!(route(&c, &presets), Ok(Route::Preset(&presets[0])));
    assert_eq!(route(&c, &presets).unwrap().settings(), Some(&presets[0].settings));
    // An unknown name is an error, never a direct connection; names match exactly.
    c.tunnel = Some("Office".into());
    assert_eq!(route(&c, &presets), Err(RouteError::NotFound("Office".into())));
    c.ssh = None;
    c.tunnel = Some("gone".into());
    assert_eq!(route(&c, &[]), Err(RouteError::NotFound("gone".into())));
}
