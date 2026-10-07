use super::*;
use datarig_core::profile::ConnectionConfig;

const MYSQL_84: Server = Server { version: (8, 4, 6), mariadb: false };

#[test]
fn versions_before_mysql_8_and_mariadb_10_6_are_refused_with_their_name() {
    let refused = |version, mariadb| Server { version, mariadb }.check().err();
    assert_eq!(
        refused((5, 7, 44), false),
        Some(DbError::VersionUnsupported { server: "MySQL 5.7.44".into(), needed: "MySQL 8.0".into() })
    );
    assert_eq!(
        refused((10, 5, 27), true),
        Some(DbError::VersionUnsupported { server: "MariaDB 10.5.27".into(), needed: "MariaDB 10.6".into() })
    );
    assert_eq!(
        refused((0, 0, 0), false),
        Some(DbError::VersionUnsupported { server: "MySQL (unknown version)".into(), needed: "MySQL 8.0".into() })
    );
    for (version, mariadb) in [((8, 0, 0), false), ((8, 4, 6), false), ((9, 7, 2), false), ((10, 6, 0), true)] {
        assert_eq!(refused(version, mariadb), None, "{version:?}");
    }
}

#[test]
fn one_set_sets_the_session_up_by_role() {
    let q = Settings { role: SessionRole::Query, read_only: false, select_limit: Some(501) };
    assert_eq!(
        init_sql(&MYSQL_84, &q),
        "SET NAMES utf8mb4, SESSION session_track_system_variables = 'sql_mode,transaction_read_only,sql_select_limit', \
         SESSION session_track_schema = ON, SESSION sql_mode = @@SESSION.sql_mode, SESSION sql_select_limit = 501"
    );
    let ro = Settings { read_only: true, ..q };
    assert!(init_sql(&MYSQL_84, &ro).ends_with(", SESSION sql_select_limit = 501, SESSION transaction_read_only = ON"));
    let meta = Settings { role: SessionRole::Meta, read_only: false, select_limit: None };
    assert!(
        init_sql(&MYSQL_84, &meta).ends_with(
            ", SESSION lock_wait_timeout = 2, SESSION max_execution_time = 10000, SESSION transaction_read_only = ON"
        ),
        "{}",
        init_sql(&MYSQL_84, &meta)
    );
    let maria = Server { version: (10, 11, 6), mariadb: true };
    let sql = init_sql(&maria, &meta);
    assert!(sql.contains("'sql_mode,tx_read_only,sql_select_limit'"), "{sql}");
    assert!(sql.ends_with(", SESSION max_statement_time = 10, SESSION tx_read_only = ON"), "{sql}");
    let maria = Server { version: (11, 4, 2), mariadb: true };
    assert!(init_sql(&maria, &ro).ends_with(", SESSION transaction_read_only = ON"));
}

#[test]
fn the_mode_follows_the_sql_mode_and_the_version() {
    let tracked = |mode: &str| Tracked { sql_mode: Some(mode.into()), ..Tracked::default() };
    assert_eq!(tracked("").mode(&MYSQL_84), MySqlMode { dollar_quotes: true, ..MySqlMode::default() });
    let ansi = tracked("REAL_AS_FLOAT,PIPES_AS_CONCAT,ANSI_QUOTES,IGNORE_SPACE,ONLY_FULL_GROUP_BY,ANSI");
    assert!(ansi.mode(&MYSQL_84).ansi_quotes);
    assert!(!ansi.mode(&MYSQL_84).no_backslash_escapes);
    assert!(tracked("STRICT_TRANS_TABLES,NO_BACKSLASH_ESCAPES").mode(&MYSQL_84).no_backslash_escapes);
    // A flag is matched whole.
    assert!(!tracked("NO_ANSI_QUOTES_X").mode(&MYSQL_84).ansi_quotes);
    let mysql80 = Server { version: (8, 0, 45), mariadb: false };
    assert!(!tracked("").mode(&mysql80).dollar_quotes);
    assert!(!tracked("").mode(&Server { version: (11, 4, 2), mariadb: true }).dollar_quotes);
}

#[test]
fn the_target_comes_from_the_fields_or_a_mysql_url() {
    let cfg = ConnectionConfig {
        driver: "mysql".into(),
        host: "db".into(),
        port: 3307,
        user: "u".into(),
        password: "pw".into(),
        database: String::new(),
        ..ConnectionConfig::default()
    };
    let t = Target::of(&cfg).unwrap();
    assert_eq!((t.host.as_str(), t.port, t.database.as_deref()), ("db", 3307, None));
    let url = |dsn: &str| ConnectionConfig { dsn: Some(dsn.into()), password: String::new(), ..cfg.clone() };
    let t = Target::of(&url("mysql://a:secret@h/shop")).unwrap();
    assert_eq!((t.host.as_str(), t.port, t.password.as_str()), ("h", 3306, "secret"));
    assert_eq!(t.database.as_deref(), Some("shop"));
    assert!(matches!(Target::of(&url("postgres://h/db")), Err(DbError::Settings(_))));
    assert!(matches!(Target::of(&url("mysql://h/db?ssl-mode=REQUIRED")), Err(DbError::Settings(_))));
}

#[test]
fn server_errors_read_as_the_mysql_client_shows_them() {
    let server = |code, state: &str, message: &str| {
        mysql_async::Error::Server(mysql_async::ServerError {
            code,
            message: message.to_string(),
            state: state.to_string(),
        })
    };
    assert_eq!(
        my_error(&server(1146, "42S02", "Table 'shop.x' doesn't exist")),
        DbError::Server("ERROR 1146 (42S02): Table 'shop.x' doesn't exist".into())
    );
    assert_eq!(
        my_error(&server(3159, "HY000", "Connections using insecure transport are prohibited")),
        DbError::TlsRequired
    );
    assert!(connect_error(&server(1045, "28000", "Access denied for user 'u'@'h' (using password: YES)")).1);
    assert!(!connect_error(&server(1049, "42000", "Unknown database 'x'")).1);
}

/// The first packet a MySQL 8.4 server sends (protocol 10), offering every capability.
fn server_greeting() -> Vec<u8> {
    let mut p = vec![10];
    p.extend_from_slice(b"8.4.0\0");
    p.extend_from_slice(&7u32.to_le_bytes());
    p.extend_from_slice(b"abcdefgh");
    p.push(0);
    p.extend_from_slice(&[0xff, 0xff]);
    p.push(255);
    p.extend_from_slice(&2u16.to_le_bytes());
    p.extend_from_slice(&[0xff, 0xff]);
    p.push(21);
    p.extend_from_slice(&[0; 10]);
    p.extend_from_slice(b"ijklmnopqrst\0");
    p.extend_from_slice(b"caching_sha2_password\0");
    let mut packet = vec![p.len() as u8, (p.len() >> 8) as u8, 0, 0];
    packet.extend_from_slice(&p);
    packet
}

#[tokio::test]
async fn the_client_never_asks_for_multi_statements_nor_local_files() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let cfg = ConnectionConfig {
        driver: "mysql".into(),
        user: "u".into(),
        password: "s3cret-Pass-zz".into(),
        ..Default::default()
    };
    let opts = Target::of(&cfg).unwrap().opts(None, "datarig-q-test");
    let (client, mut server) = tokio::io::duplex(1 << 16);
    let login = tokio::spawn(Conn::connect_with_stream(opts, Box::new(client)));
    server.write_all(&server_greeting()).await.unwrap();
    let mut header = [0u8; 4];
    server.read_exact(&mut header).await.unwrap();
    let len = usize::from(header[0]) | usize::from(header[1]) << 8 | usize::from(header[2]) << 16;
    let mut response = vec![0; len];
    server.read_exact(&mut response).await.unwrap();
    let caps = u32::from_le_bytes(response[..4].try_into().unwrap());
    const MULTI_STATEMENTS: u32 = 1 << 16;
    const LOCAL_FILES: u32 = 1 << 7;
    const SESSION_TRACK: u32 = 1 << 23;
    const CONNECT_ATTRS: u32 = 1 << 20;
    assert_eq!(caps & MULTI_STATEMENTS, 0, "{caps:#x}");
    assert_eq!(caps & LOCAL_FILES, 0, "{caps:#x}");
    assert_ne!(caps & SESSION_TRACK, 0, "{caps:#x}");
    assert_ne!(caps & CONNECT_ATTRS, 0, "the program name goes with the login: {caps:#x}");
    // The password itself is not in the response (a scramble of it is).
    assert!(!response.windows(14).any(|w| w == b"s3cret-Pass-zz"));
    drop(server);
    assert!(login.await.unwrap().is_err());
}
