use super::*;

#[test]
fn loopback_hosts_are_this_machine() {
    for h in ["localhost", "LOCALHOST", "127.0.0.1", "127.8.9.10", "::1", "[::1]", "::ffff:127.0.0.1"] {
        assert!(is_loopback(h), "{h}");
    }
    for h in ["", "db", "localhost.example", "10.0.0.5", "128.0.0.1", "::2", "0.0.0.0", "my-localhost"] {
        assert!(!is_loopback(h), "{h}");
    }
}

#[test]
fn a_mysql_profile_is_unencrypted_when_it_connects_directly_to_another_machine() {
    let my = ConnectionConfig { driver: "mysql".into(), host: "db.example".into(), ..ConnectionConfig::default() };
    assert!(my.mysql_unencrypted());
    assert!(ConnectionConfig { driver: "mariadb".into(), ..my.clone() }.mysql_unencrypted());
    // PostgreSQL has its own TLS settings.
    assert!(!ConnectionConfig { driver: "postgres".into(), ..my.clone() }.mysql_unencrypted());
    for host in ["localhost", "127.0.0.1", "::1"] {
        assert!(!ConnectionConfig { host: host.into(), ..my.clone() }.mysql_unencrypted(), "{host}");
    }
    // Through an SSH tunnel: a preset, or the profile's own one while it is on.
    assert!(!ConnectionConfig { tunnel: Some("bastion".into()), ..my.clone() }.mysql_unencrypted());
    let own = ssh::SshSettings { enabled: true, host: "b".into(), user: "u".into(), ..ssh::SshSettings::default() };
    assert!(!ConnectionConfig { ssh: Some(own.clone()), ..my.clone() }.mysql_unencrypted());
    let off = ssh::SshSettings { enabled: false, ..own };
    assert!(ConnectionConfig { ssh: Some(off), ..my.clone() }.mysql_unencrypted());
    // The host of a URL the profile keeps (none: `localhost`).
    let url = |d: &str| ConnectionConfig { dsn: Some(d.into()), host: "127.0.0.1".into(), ..my.clone() };
    assert!(url("mysql://u@db.example/shop?x=1").mysql_unencrypted());
    assert!(!url("mysql://u@127.0.0.1/shop?x=1").mysql_unencrypted());
    assert!(!url("mysql://u@/shop?x=1").mysql_unencrypted());
}
