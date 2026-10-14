use super::*;

#[test]
fn read_only_goes_after_the_dsn_options() {
    let mut cfg = Config::new();
    read_only(&mut cfg);
    assert_eq!(cfg.get_options(), Some(READ_ONLY_OPTION));
    let mut cfg = Config::from_str("postgres://u@h/d?options=-c%20search_path%3Dshop").unwrap();
    read_only(&mut cfg);
    assert_eq!(cfg.get_options(), Some("-c search_path=shop -c default_transaction_read_only=on"));
    let mut cfg = Config::from_str("postgres://u@h/d?options=-c%20default_transaction_read_only%3Doff").unwrap();
    read_only(&mut cfg);
    let options = cfg.get_options().unwrap();
    assert!(options.ends_with(READ_ONLY_OPTION), "the last setting wins: {options}");
}

#[test]
fn a_schema_is_quoted_and_escaped_as_one_startup_option() {
    assert_eq!(search_path("shop"), "\"shop\", public");
    assert_eq!(search_path("public"), "\"public\"");
    assert_eq!(search_path("My \"x\""), "\"My \"\"x\"\"\", public");
    assert_eq!(option_escape("\"a b\\c\""), "\"a\\ b\\\\c\"");
    let mut cfg = tokio_postgres::Config::new();
    cfg.options("-c statement_timeout=5s");
    add_option(&mut cfg, &format!("-c search_path={}", option_escape(&search_path("a b"))));
    assert_eq!(cfg.get_options(), Some("-c statement_timeout=5s -c search_path=\"a\\ b\",\\ public"));
}

#[test]
fn a_dsns_tls_parameters_are_left_out_of_what_tokio_postgres_reads() {
    let cases = [
        (
            "postgres://u@h/db?sslmode=verify-full&application_name=x&sslrootcert=%2Fca.pem",
            "postgres://u@h/db?application_name=x",
        ),
        ("postgres://u@h1,h2/db?ssl%6dode=require", "postgres://u@h1,h2/db"),
        ("postgres://u@h/db", "postgres://u@h/db"),
        ("host=h sslmode='verify-full' sslrootcert=/ca.pem dbname=x", "host=h   dbname=x"),
        ("host=h dbname=x", "host=h dbname=x"),
    ];
    for (dsn, kept) in cases {
        assert_eq!(without_tls_params(dsn), kept, "{dsn}");
        // What is left still parses.
        assert!(Config::from_str(&without_tls_params(dsn)).is_ok(), "{dsn}");
    }
}

#[test]
fn the_tls_settings_reach_the_connection_settings() {
    use datarig_core::tls::TlsProblem;
    let mode = |c: &ConnectionConfig| pg_config(c).map(|(cfg, _)| cfg.get_ssl_mode());
    let base = ConnectionConfig::test_db();
    assert_eq!(mode(&base), Ok(SslMode::Disable));
    for (m, want) in [("prefer", SslMode::Prefer), ("require", SslMode::Require), ("verify-full", SslMode::Require)] {
        assert_eq!(mode(&ConnectionConfig { sslmode: m.into(), ..base.clone() }), Ok(want), "{m}");
    }
    // A Unix socket: never TLS (PostgreSQL has none there).
    let socket = ConnectionConfig { host: "/var/run/postgresql".into(), sslmode: "require".into(), ..base.clone() };
    assert_eq!(mode(&socket), Ok(SslMode::Disable));
    // A DSN's own mode, also one tokio-postgres would not read.
    let dsn = ConnectionConfig { dsn: Some("postgres://u@h/db?sslmode=verify-full".into()), ..base.clone() };
    assert_eq!(mode(&dsn), Ok(SslMode::Require));
    let bad = ConnectionConfig { sslmode: "verify".into(), ..base };
    assert_eq!(mode(&bad), Err(DbError::Tls(TlsProblem::UnknownMode("verify".into()))));
}
