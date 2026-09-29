use super::*;

#[test]
fn parses_names_and_hex() {
    assert_eq!(ProfileColor::parse("red"), Some(ProfileColor::Named(0)));
    assert_eq!(ProfileColor::parse(" Gray "), Some(ProfileColor::Named(11)));
    assert_eq!(ProfileColor::parse("#1a2B3c"), Some(ProfileColor::Hex(0x1a, 0x2b, 0x3c)));
    for bad in ["", "reddish", "#12345", "#1234567", "#12345g", "1a2b3c"] {
        assert_eq!(ProfileColor::parse(bad), None, "{bad:?}");
    }
    assert_eq!(ProfileColor::Hex(0x1a, 0x2b, 0x3c).to_string(), "#1a2b3c");
    assert_eq!(ProfileColor::Named(6).to_string(), "blue");
    for n in NAMES {
        assert_eq!(ProfileColor::parse(n).unwrap().to_string(), n);
    }
}

#[test]
fn automatic_color_is_a_fixed_hash_of_the_name() {
    // FNV-1a 32: fixed values, so a profile keeps its color across runs, platforms and versions.
    assert_eq!(ProfileColor::auto(""), ProfileColor::Named((0x811c_9dc5u32 % 12) as u8));
    assert_eq!(ProfileColor::auto("a"), ProfileColor::Named((0xe40c_292cu32 % 12) as u8));
    assert_eq!(ProfileColor::auto("local-pg"), ProfileColor::auto("local-pg"));
    let spread: std::collections::BTreeSet<_> =
        ["prod", "staging", "dev", "local-pg", "分析-replica", "v6", "shard-1", "shard-2"]
            .iter()
            .map(|n| ProfileColor::auto(n))
            .collect();
    assert!(spread.len() >= 4, "{spread:?}");
}
