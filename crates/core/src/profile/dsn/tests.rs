use super::*;

fn dsn(user: &str, pw: Option<&str>, host: &str, port: Option<u16>, db: &str, params: &[(&str, &str)]) -> Dsn {
    Dsn {
        scheme: Scheme::Postgres,
        user: user.into(),
        password: pw.map(Into::into),
        host: host.into(),
        port,
        database: db.into(),
        params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

#[test]
fn debug_never_leaks_password() {
    let d = dsn("u", Some("hunter2"), "h", None, "db", &[]);
    let out = format!("{d:?}");
    assert!(!out.contains("hunter2"), "{out}");
    assert!(out.contains("<redacted>"), "{out}");

    assert!(format!("{:?}", dsn("u", Some(""), "h", None, "db", &[])).contains("<unset>"));
    assert!(format!("{:?}", dsn("u", None, "h", None, "db", &[])).contains("None"));
}

#[test]
fn parses_common_forms() {
    assert_eq!(
        parse("postgres://datarig@127.0.0.1:55432/datarig?sslmode=disable").unwrap(),
        dsn("datarig", None, "127.0.0.1", Some(55432), "datarig", &[("sslmode", "disable")])
    );
    assert_eq!(parse("postgresql://db.example.com/app").unwrap(), dsn("", None, "db.example.com", None, "app", &[]));
    assert_eq!(parse("  POSTGRES://u:p@h  ").unwrap(), dsn("u", Some("p"), "h", None, "", &[]));
    assert_eq!(parse("postgres://h:5432").unwrap(), dsn("", None, "h", Some(5432), "", &[]));
    // Empty password is still a password (`user:@host`).
    assert_eq!(parse("postgres://u:@h/d").unwrap().password.as_deref(), Some(""));
}

#[test]
fn ipv6_hosts() {
    let d = parse("postgres://u@[::1]:5433/db").unwrap();
    assert_eq!((d.host.as_str(), d.port), ("::1", Some(5433)));
    let d = parse("postgres://[2001:db8::7]/db").unwrap();
    assert_eq!((d.host.as_str(), d.port), ("2001:db8::7", None));
    assert_eq!(format(&d), "postgres://[2001:db8::7]/db");
    assert!(matches!(parse("postgres://[::1/db"), Err(DsnError::Host(_))));
    assert!(matches!(parse("postgres://::1/db"), Err(DsnError::Host(_))));
    assert!(matches!(parse("postgres://[::1]x/db"), Err(DsnError::Host(_))));
}

#[test]
fn percent_encoding() {
    let d = parse("postgres://us%40er:p%40ss%3Aw%2Frd%20%F0%9F%90%98@h/%E4%B8%AD%E6%96%87db").unwrap();
    assert_eq!(d.user, "us@er");
    assert_eq!(d.password.as_deref(), Some("p@ss:w/rd 🐘"));
    assert_eq!(d.database, "中文db");
    // Unencoded '@' in the password: the last '@' separates userinfo from host.
    let d = parse("postgres://u:a@b@host/db").unwrap();
    assert_eq!((d.password.as_deref(), d.host.as_str()), (Some("a@b"), "host"));
    assert_eq!(parse("postgres://u%4@h"), Err(DsnError::Encoding));
    assert_eq!(parse("postgres://u%zz@h"), Err(DsnError::Encoding));
    assert_eq!(parse("postgres://%FF@h"), Err(DsnError::Encoding));
}

#[test]
fn errors() {
    assert_eq!(parse("redis://h/db"), Err(DsnError::Scheme));
    assert_eq!(parse("host=localhost port=5432"), Err(DsnError::Scheme));
    assert_eq!(parse("postgres://h:abc/db"), Err(DsnError::Port("abc".into())));
    assert_eq!(parse("postgres://h:0/db"), Err(DsnError::Port("0".into())));
    assert_eq!(parse("postgres://h:70000/db"), Err(DsnError::Port("70000".into())));
    assert_eq!(parse("postgres://h:/db"), Err(DsnError::Port(String::new())));
    assert!(matches!(parse("postgres://h1,h2/db"), Err(DsnError::Host(_))));
    assert!(matches!(parse("postgres://h1:1:2/db"), Err(DsnError::Host(_))));
}

#[test]
fn query_params() {
    let d = parse("postgres://h/db?sslmode=require&application_name=data%20rig&flag").unwrap();
    assert_eq!(d.param("sslmode"), Some("require"));
    assert_eq!(d.param("application_name"), Some("data rig"));
    assert_eq!(d.param("flag"), Some(""));
    assert_eq!(d.param("missing"), None);
    // Params without a database path.
    let d = parse("postgres://h?sslmode=disable").unwrap();
    assert_eq!((d.database.as_str(), d.param("sslmode")), ("", Some("disable")));
    assert_eq!(format(&d), "postgres://h/?sslmode=disable");
}

#[test]
fn roundtrip() {
    let cases = [
        dsn("datarig", None, "127.0.0.1", Some(55432), "datarig", &[("sslmode", "disable")]),
        dsn("us@er", Some("p@ss:w/rd 🐘"), "::1", Some(5432), "中文 db", &[("sslmode", "require")]),
        dsn("", None, "localhost", None, "", &[]),
        dsn("u", Some(""), "h", None, "d", &[("a", "1&2=3"), ("b", "")]),
        dsn("陳", None, "fe80::1%lo0", Some(1), "x/y", &[]),
    ];
    for d in cases {
        let s = format(&d);
        assert_eq!(parse(&s).unwrap(), d, "roundtrip through {s}");
        assert_eq!(format(&parse(&s).unwrap()), s);
    }
}

#[test]
fn secret_span_masks_only_password() {
    let s = "postgres://user:hunter2@host:5432/db";
    let (a, b) = secret_span(s).unwrap();
    assert_eq!(&s[a..b], "hunter2");
    assert_eq!(secret_span("postgres://user@host:5432/db"), None);
    assert_eq!(secret_span("postgres://host:5432/db"), None);
    let s = " postgres://u:a@b@h/d";
    let (a, b) = secret_span(s).unwrap();
    assert_eq!(&s[a..b], "a@b");
}

#[test]
fn mysql_urls_have_their_scheme() {
    let d = parse("mysql://datarig:pw@127.0.0.1:53306/shop").unwrap();
    assert_eq!(d.scheme, Scheme::MySql);
    assert_eq!((d.user.as_str(), d.password.as_deref(), d.port), ("datarig", Some("pw"), Some(53306)));
    assert_eq!(d.database, "shop");
    assert_eq!(parse("MariaDB://h").unwrap().scheme, Scheme::MySql);
    assert_eq!(parse("postgresql://h").unwrap().scheme, Scheme::Postgres);
    // Written back with the scheme it was read with (`mariadb://` as `mysql://`).
    assert_eq!(format(&parse("mariadb://u@h:3307/db").unwrap()), "mysql://u@h:3307/db");
    assert_eq!(format(&parse("postgres://u@h/db").unwrap()), "postgres://u@h/db");
    assert_eq!(parse("redis://h"), Err(DsnError::Scheme));
}

#[test]
fn schemes_of_drivers() {
    for (driver, scheme) in [
        ("postgres", Scheme::Postgres),
        ("pg", Scheme::Postgres),
        ("PostgreSQL", Scheme::Postgres),
        ("mysql", Scheme::MySql),
        ("MariaDB", Scheme::MySql),
    ] {
        assert_eq!(Scheme::of_driver(driver), scheme, "{driver}");
    }
    assert_eq!((Scheme::Postgres.default_port(), Scheme::MySql.default_port()), (5432, 3306));
}

#[test]
fn key_value_connection_strings_are_read_as_libpq_reads_them() {
    let text = "host=db.example port = 5432  sslrootcert='/etc/my certs/ca.pem' password=a\\ b sslmode=verify-full";
    let pairs = conninfo(text).unwrap();
    let kv: Vec<_> = pairs.iter().map(|p| (p.key.as_str(), p.value.as_str())).collect();
    assert_eq!(
        kv,
        [
            ("host", "db.example"),
            ("port", "5432"),
            ("sslrootcert", "/etc/my certs/ca.pem"),
            ("password", "a b"),
            ("sslmode", "verify-full")
        ]
    );
    // Each span covers its pair, so the text without one is the others as written.
    assert_eq!(&text[pairs[1].span.clone()], "port = 5432");
    assert_eq!(&text[pairs[2].span.clone()], "sslrootcert='/etc/my certs/ca.pem'");
    assert_eq!(conninfo("a='it\\'s' b=''").unwrap()[0].value, "it's");
    assert_eq!(conninfo("b=''").unwrap()[0].value, "");
    assert_eq!(conninfo("   ").unwrap(), []);
    // Not a connection string.
    for bad in ["host", "host=x port", "=x", "a='open", "a='x\\"] {
        assert_eq!(conninfo(bad), None, "{bad}");
    }
}
