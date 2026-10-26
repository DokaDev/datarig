use super::*;
use std::time::Duration;

fn t(v: toml::Value) -> Result<Option<Duration>, String> {
    parse_timeout(&v)
}

#[test]
fn timeouts_parse_with_units_and_off() {
    use toml::Value::{Integer, String as S};
    assert_eq!(t(Integer(45)), Ok(Some(Duration::from_secs(45))));
    assert_eq!(t(S("30s".into())), Ok(Some(Duration::from_secs(30))));
    assert_eq!(t(S("10".into())), Ok(Some(Duration::from_secs(10))));
    assert_eq!(t(S("5m".into())), Ok(Some(Duration::from_secs(300))));
    assert_eq!(t(S("1h".into())), Ok(Some(Duration::from_secs(3600))));
    assert_eq!(t(S("off".into())), Ok(None));
    assert_eq!(t(S("OFF".into())), Ok(None));
    assert_eq!(t(Integer(0)), Ok(None), "zero disables it");
    assert_eq!(t(S("0s".into())), Ok(None));
    for bad in ["", "s", "10x", "1.5m", "-3", "ten"] {
        assert!(t(S(bad.into())).is_err(), "{bad:?}");
    }
    assert!(t(Integer(-1)).is_err());
    assert!(t(toml::Value::Boolean(true)).is_err());
}

#[test]
fn profiles_get_their_environment_or_the_default_rules() {
    let mut p = Policies::default();
    assert_eq!(p.get(None).unwrap().paging_idle_timeout, Some(PAGING_IDLE_TIMEOUT), "built-in default: 30s");
    p.insert("careful", Policy { paging_idle_timeout: Some(Duration::from_secs(10)), ..Policy::default() });
    p.insert("default", Policy { paging_idle_timeout: None, ..Policy::default() });
    assert_eq!(p.get(Some("careful")).unwrap().paging_idle_timeout, Some(Duration::from_secs(10)));
    assert_eq!(p.get(None).unwrap().paging_idle_timeout, None, "[policy.default] changes the default rules");
    assert_eq!(p.get(Some("default")).unwrap().paging_idle_timeout, None, "named, the default rules too");
    let names: Vec<&str> = p.environments().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["careful"], "the default rules are no environment");
}

#[test]
fn an_unknown_environment_is_an_error_never_the_default_rules() {
    let mut p = Policies::default();
    p.insert("default", Policy { read_only: true, ..Policy::default() });
    assert_eq!(p.get(Some("prod")), Err(UnknownEnvironment("prod".into())));
    assert_eq!(p.get(Some("Default")), Err(UnknownEnvironment("Default".into())), "names are exact");
    p.insert("prod", Policy::default());
    assert!(p.get(Some("prod")).is_ok());
}

#[test]
fn badges() {
    for ok in ["PROD", "STG", "dev", "12345678", "\u{c6b4}\u{c601}", "a b"] {
        assert!(badge_ok(ok), "{ok:?}");
    }
    // Wide characters take two columns each: five of them are ten.
    for bad in ["", " PROD", "PROD ", "123456789", "a\tb", "\u{c6b4}\u{c601}\u{c6b4}\u{c601}\u{c6b4}"] {
        assert!(!badge_ok(bad), "{bad:?}");
    }
}

#[test]
fn the_presets() {
    let p = presets();
    let names: Vec<&str> = p.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["dev", "test", "staging", "prod"]);
    let prod = &p[3];
    assert!(prod.policy.read_only && prod.policy.confirm.writes());
    assert_eq!((prod.color.as_deref(), prod.badge.as_deref()), (Some("red"), Some("PROD")));
    assert_eq!(
        (p[2].policy.read_only, p[2].policy.confirm, p[2].badge.as_deref()),
        (false, Confirm::Writes, Some("STG"))
    );
    // Staging and prod commit by hand; dev and test (the default rules) commit by themselves.
    let commits: Vec<CommitRule> = p.iter().map(|e| e.policy.commit).collect();
    assert_eq!(commits, [CommitRule::Auto, CommitRule::Auto, CommitRule::Manual, CommitRule::Manual]);
    assert_eq!(
        (p[0].policy.clone(), p[0].color.as_deref(), p[0].badge.clone()),
        (Policy::default(), Some("green"), None)
    );
    assert_eq!(
        (p[1].policy.clone(), p[1].color.as_deref(), p[1].badge.clone()),
        (Policy::default(), Some("blue"), None)
    );
    assert!(p.iter().all(|e| e.display_color().is_some() && e.badge.as_deref().is_none_or(badge_ok)));
    assert_ne!(p[0].id, presets()[0].id, "each its own id");
}

#[test]
fn values_are_written_as_they_are_read() {
    for (t, text) in [(Some(30), "30s"), (Some(300), "5m"), (Some(3600), "1h"), (Some(90), "90s"), (None, "off")] {
        let d = t.map(Duration::from_secs);
        assert_eq!(format_timeout(d), text);
        assert_eq!(parse_timeout(&toml::Value::String(text.into())), Ok(d));
    }
    for (l, text) in [
        (Some(1 << 30), "1GB"),
        (Some(512 << 20), "512MB"),
        (Some(64 << 10), "64KB"),
        (Some(1000), "1000"),
        (None, "off"),
    ] {
        assert_eq!(format_size(SpillLimit(l)), text);
        assert_eq!(parse_size(&toml::Value::String(text.into())), Ok(SpillLimit(l)));
    }
    assert_eq!((paging_str(PagingMode::Hold), paging_str(PagingMode::NoHold)), ("hold", "no_hold"));
    assert_eq!((Confirm::Writes.as_str(), Confirm::Destructive.as_str()), ("writes", "destructive"));
}

#[test]
fn paging_modes() {
    assert_eq!(parse_paging("no_hold"), Some(PagingMode::NoHold));
    assert_eq!(parse_paging(" HOLD "), Some(PagingMode::Hold));
    for bad in ["", "nohold", "no-hold", "keep", "off"] {
        assert_eq!(parse_paging(bad), None, "{bad:?}");
    }
    assert_eq!(Policy::default().paging, PagingMode::NoHold, "nothing is held by default");
}

#[test]
fn spill_limits() {
    let v = |s: &str| toml::Value::String(s.into());
    assert_eq!(parse_size(&v("512MB")), Ok(SpillLimit(Some(512 << 20))));
    assert_eq!(parse_size(&v("2 gb")), Ok(SpillLimit(Some(2 << 30))));
    assert_eq!(parse_size(&v("64KB")), Ok(SpillLimit(Some(64 << 10))));
    assert_eq!(parse_size(&toml::Value::Integer(1000)), Ok(SpillLimit(Some(1000))));
    assert_eq!(parse_size(&v("off")), Ok(SpillLimit(None)));
    assert_eq!(parse_size(&toml::Value::Integer(0)), Ok(SpillLimit(None)));
    for bad in [v("lots"), v("5TB"), v("-1MB"), toml::Value::Integer(-1), toml::Value::Boolean(true)] {
        assert!(parse_size(&bad).is_err(), "{bad:?}");
    }
    assert_eq!(SpillLimit::default(), SpillLimit(Some(1 << 30)));
}

#[test]
fn confirm_levels() {
    assert_eq!(Confirm::parse("destructive"), Some(Confirm::Destructive));
    assert_eq!(Confirm::parse(" Writes "), Some(Confirm::Writes));
    assert_eq!(Confirm::parse("off"), None, "destructive statements always ask");
    assert_eq!(Confirm::parse(""), None);
    assert!(Confirm::Writes.writes() && !Confirm::Destructive.writes());
    let p = Policy::default();
    assert_eq!((p.read_only, p.confirm), (false, Confirm::Destructive));
}

#[test]
fn commit_rules() {
    assert_eq!(CommitRule::parse("auto"), Some(CommitRule::Auto));
    assert_eq!(CommitRule::parse(" Manual "), Some(CommitRule::Manual));
    for bad in ["", "off", "manually", "on", "true"] {
        assert_eq!(CommitRule::parse(bad), None, "{bad:?}");
    }
    assert_eq!((CommitRule::Auto.as_str(), CommitRule::Manual.as_str()), ("auto", "manual"));
    assert!(CommitRule::Manual.manual() && !CommitRule::Auto.manual());
    let p = Policy::default();
    assert_eq!(p.commit, CommitRule::Auto, "a missing key commits by itself, as before");
    assert_eq!((p.tx_idle_warning, p.tx_idle_rollback), (Some(TX_IDLE_WARNING), Some(TX_IDLE_ROLLBACK)));
    assert_eq!((TX_IDLE_WARNING.as_secs(), TX_IDLE_ROLLBACK.as_secs()), (300, 1800));
}

#[test]
fn transaction_idle_times_have_a_floor() {
    let v = |s: &str| toml::Value::String(s.into());
    assert_eq!(parse_tx_idle(&v("5m")), Ok(Some(Duration::from_secs(300))));
    assert_eq!(parse_tx_idle(&v("10s")), Ok(Some(TX_IDLE_FLOOR)), "the floor itself");
    assert_eq!(parse_tx_idle(&toml::Value::Integer(90)), Ok(Some(Duration::from_secs(90))));
    assert_eq!(parse_tx_idle(&v("off")), Ok(None));
    assert_eq!(parse_tx_idle(&toml::Value::Integer(0)), Ok(None), "zero is off, as for paging");
    for bad in
        [v("9s"), v("1"), toml::Value::Integer(5), v("soon"), v("-1m"), v("5 minutes"), toml::Value::Boolean(true)]
    {
        assert!(parse_tx_idle(&bad).is_err(), "{bad:?}");
    }
    assert!(parse_tx_idle(&v("9s")).unwrap_err().contains("10s"), "says the floor");
    assert_eq!(parse_tx_idle_text("30m"), Ok(Some(TX_IDLE_ROLLBACK)));
    assert!(parse_tx_idle_text("3s").is_err());
}
