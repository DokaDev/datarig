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
fn a_profile_is_exposed_when_it_connects_directly_to_another_machine() {
    let my = ConnectionConfig { driver: "mysql".into(), host: "db.example".into(), ..ConnectionConfig::default() };
    assert!(my.exposed());
    assert!(ConnectionConfig { driver: "mariadb".into(), ..my.clone() }.exposed());
    assert!(ConnectionConfig { driver: "postgres".into(), ..my.clone() }.exposed());
    for host in ["localhost", "127.0.0.1", "::1"] {
        assert!(!ConnectionConfig { host: host.into(), ..my.clone() }.exposed(), "{host}");
    }
    // Through an SSH tunnel: a preset, or the profile's own one while it is on.
    assert!(!ConnectionConfig { tunnel: Some("bastion".into()), ..my.clone() }.exposed());
    let own = ssh::SshSettings { enabled: true, host: "b".into(), user: "u".into(), ..ssh::SshSettings::default() };
    assert!(!ConnectionConfig { ssh: Some(own.clone()), ..my.clone() }.exposed());
    let off = ssh::SshSettings { enabled: false, ..own };
    assert!(ConnectionConfig { ssh: Some(off), ..my.clone() }.exposed());
    // The host of a URL the profile keeps (none: `localhost`).
    let url = |d: &str| ConnectionConfig { dsn: Some(d.into()), host: "127.0.0.1".into(), ..my.clone() };
    assert!(url("mysql://u@db.example/shop?x=1").exposed());
    assert!(!url("mysql://u@127.0.0.1/shop?x=1").exposed());
    assert!(!url("mysql://u@/shop?x=1").exposed());
}

#[test]
fn a_profile_is_surely_unencrypted_only_when_exposed_and_its_tls_is_off() {
    let pg = ConnectionConfig { host: "db.example".into(), ..ConnectionConfig::default() };
    // No mode: PostgreSQL's default is off; MySQL's is PREFERRED (it may be encrypted).
    assert!(pg.unencrypted());
    let my = ConnectionConfig { driver: "mysql".into(), ..pg.clone() };
    assert!(!my.unencrypted());
    assert!(ConnectionConfig { sslmode: "DISABLED".into(), ..my.clone() }.unencrypted());
    assert!(!ConnectionConfig { sslmode: "prefer".into(), ..pg.clone() }.unencrypted());
    assert!(!ConnectionConfig { host: "localhost".into(), ..pg.clone() }.unencrypted());
}

#[test]
fn tls_settings_come_from_the_fields_or_a_postgres_dsn() {
    let pg = ConnectionConfig {
        sslmode: "verify-full".into(),
        sslrootcert: Some("/ca.pem".into()),
        ..ConnectionConfig::default()
    };
    let t = pg.tls().unwrap();
    assert_eq!((t.mode, t.ca_file.as_deref()), (TlsMode::VerifyFull, Some("/ca.pem")));
    // A DSN's parameters win over the fields of the same name; the others stay.
    let url = ConnectionConfig {
        dsn: Some("postgres://u@h/db?sslmode=require&sslcert=/c.pem&sslkey=/c.key&application_name=x".into()),
        ..pg.clone()
    };
    let t = url.tls().unwrap();
    // `require` with a CA file (the field's) checks the chain.
    assert_eq!(
        (t.mode, t.ca_file.as_deref(), t.cert_file.as_deref(), t.key_file.as_deref()),
        (TlsMode::VerifyCa, Some("/ca.pem"), Some("/c.pem"), Some("/c.key"))
    );
    let kv = ConnectionConfig { dsn: Some("host=h sslmode=prefer sslrootcert=''".into()), ..pg.clone() };
    let t = kv.tls().unwrap();
    assert_eq!((t.mode, t.ca_file), (TlsMode::Prefer, None));
    let bad = ConnectionConfig { dsn: Some("host=h sslmode=verify".into()), ..pg.clone() };
    assert_eq!(bad.tls(), Err(TlsProblem::UnknownMode("verify".into())));
    // MySQL's come from its fields; a pinned server key keeps its old default.
    let my = ConnectionConfig { driver: "mysql".into(), ..ConnectionConfig::default() };
    assert_eq!(my.tls().unwrap().mode, TlsMode::Prefer);
    let pinned = ConnectionConfig { server_public_key_file: Some("/k.pem".into()), ..my.clone() };
    assert_eq!(pinned.tls().unwrap().mode, TlsMode::Disable);
}

#[test]
fn a_postgres_url_with_only_tls_parameters_becomes_fields() {
    let mut c = ConnectionConfig {
        dsn: Some("postgres://u@db:6432/app?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem".into()),
        ..ConnectionConfig::default()
    };
    c.normalize();
    assert_eq!(c.dsn, None);
    assert_eq!((c.host.as_str(), c.port, c.sslmode.as_str()), ("db", 6432, "verify-full"));
    assert_eq!(c.sslrootcert.as_deref(), Some("/etc/ca.pem"));
    assert_eq!(c.display_dsn(), "postgres://u@db:6432/app?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem");
    // Kept as written when something would be lost: another parameter, a repeated one, an
    // unknown mode, or a file the fields already name.
    for (url, field) in [
        ("postgres://h/db?sslmode=require&connect_timeout=3", None),
        ("postgres://h/db?sslcert=/a&sslcert=/b", None),
        ("postgres://h/db?sslmode=verify", None),
        ("postgres://h/db?sslkey=/a", Some("/b")),
    ] {
        let mut c = ConnectionConfig {
            dsn: Some(url.into()),
            sslkey: field.map(str::to_string),
            ..ConnectionConfig::default()
        };
        c.normalize();
        assert_eq!(c.dsn.as_deref(), Some(url));
    }
}
