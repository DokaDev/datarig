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
fn profiles_get_their_policy_or_default() {
    let mut p = Policies::default();
    assert_eq!(p.get(None).paging_idle_timeout, Some(PAGING_IDLE_TIMEOUT), "built-in default: 30s");
    assert_eq!(p.get(Some("nope")).paging_idle_timeout, Some(PAGING_IDLE_TIMEOUT));
    p.insert("careful", Policy { paging_idle_timeout: Some(Duration::from_secs(10)), ..Policy::default() });
    p.insert("default", Policy { paging_idle_timeout: None, ..Policy::default() });
    assert_eq!(p.get(Some("careful")).paging_idle_timeout, Some(Duration::from_secs(10)));
    assert_eq!(p.get(None).paging_idle_timeout, None, "[policy.default] changes the default policy");
    assert_eq!(p.get(Some("unknown")).paging_idle_timeout, None, "an unknown name gets default");
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
