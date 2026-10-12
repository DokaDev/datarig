use super::*;
use crate::route::Route;
use datarig_core::profile::ConnectionConfig;
use datarig_core::transport::{DialerRef, TcpDialer};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        "SET NAMES utf8mb4, SESSION session_track_system_variables = \
         'sql_mode,transaction_read_only,sql_select_limit,time_zone,character_set_results,character_set_connection', \
         SESSION session_track_schema = ON, SESSION sql_mode = @@SESSION.sql_mode, \
         SESSION time_zone = @@SESSION.time_zone, \
         SESSION character_set_results = @@SESSION.character_set_results, \
         SESSION character_set_connection = @@SESSION.character_set_connection, SESSION sql_select_limit = 501"
    );
    let ro = Settings { read_only: true, ..q };
    assert!(init_sql(&MYSQL_84, &ro).ends_with(", SESSION sql_select_limit = 501, SESSION transaction_read_only = ON"));
    let meta = Settings { role: SessionRole::Meta, read_only: false, select_limit: None };
    assert!(
        init_sql(&MYSQL_84, &meta).ends_with(
            ", SESSION lock_wait_timeout = 2, SESSION max_execution_time = 10000, SESSION sql_quote_show_create = 1, \
             SESSION transaction_read_only = ON"
        ),
        "{}",
        init_sql(&MYSQL_84, &meta)
    );
    let maria = Server { version: (10, 11, 6), mariadb: true };
    let sql = init_sql(&maria, &meta);
    assert!(sql.contains("'sql_mode,tx_read_only,sql_select_limit,time_zone,"), "{sql}");
    assert!(
        sql.ends_with(
            ", SESSION max_statement_time = 10, SESSION sql_quote_show_create = 1, SESSION tx_read_only = ON"
        ),
        "{sql}"
    );
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
    // A refused login may be a wrong password (a prompt can help) or an account that requires
    // TLS: the wording says both.
    let denied = "ERROR 1045 (28000): Access denied for user 'u'@'h' (using password: YES)";
    assert_eq!(
        connect_error(&server(1045, "28000", "Access denied for user 'u'@'h' (using password: YES)")),
        (DbError::AccessDenied(denied.into()), true)
    );
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
    let cfg = ConnectionConfig {
        driver: "mysql".into(),
        user: "u".into(),
        password: "s3cret-Pass-zz".into(),
        ..Default::default()
    };
    let opts = Target::of(&cfg).unwrap().opts(None, "datarig-q-test", &Route::new("127.0.0.1", 3306, None));
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

/// A 2048-bit RSA public key, as MySQL writes `public_key.pem` (SubjectPublicKeyInfo).
const KEY_2048: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAs7uesYv+jCjfQLX/mnir\nNMpG8TBaLvuQ1Fgv3j/4ipvicNTp+Ft9h7uAdntx//JbZOFe1iu/0IhP/r77pbYQ\nLwJSvozvsJ3AJW/J7ZMTLJ5c4RfciGUUz2ec7W6FKVNDqhASZmXSNhgAiEx7nEFM\nMEhuntdG/HpqvDebbleNmJlGJwpfDiGmeteYQTAv/3I965LS4njdMkV2asMPN5JV\n+v6visukUX0tlXxm/kaKMQBgyt369mAVCfQG9nRCgsQu1pmS4jZS573aCCFkv5Ql\nV9BJoVLx22kBsyDRsu9XjrotHMQNZ16kiw4Pz8IERxgau+7hzMOY40CLaaq0Gac0\ntwIDAQAB\n-----END PUBLIC KEY-----";

/// The same key as PKCS#1 (`BEGIN RSA PUBLIC KEY`).
const KEY_2048_PKCS1: &str = "-----BEGIN RSA PUBLIC KEY-----\nMIIBCgKCAQEAs7uesYv+jCjfQLX/mnirNMpG8TBaLvuQ1Fgv3j/4ipvicNTp+Ft9\nh7uAdntx//JbZOFe1iu/0IhP/r77pbYQLwJSvozvsJ3AJW/J7ZMTLJ5c4RfciGUU\nz2ec7W6FKVNDqhASZmXSNhgAiEx7nEFMMEhuntdG/HpqvDebbleNmJlGJwpfDiGm\neteYQTAv/3I965LS4njdMkV2asMPN5JV+v6visukUX0tlXxm/kaKMQBgyt369mAV\nCfQG9nRCgsQu1pmS4jZS573aCCFkv5QlV9BJoVLx22kBsyDRsu9XjrotHMQNZ16k\niw4Pz8IERxgau+7hzMOY40CLaaq0Gac0twIDAQAB\n-----END RSA PUBLIC KEY-----";

/// A 1024-bit key: too short.
const KEY_1024: &str = "-----BEGIN PUBLIC KEY-----\nMIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDAS8fAqHSwujc73UowjnpXpYzH\nEdF8DqudUhOFtsdjpNpseUgfJNUuCy49Um4Uv+wSt4qmBKZ+U/TyrdTfUuY2dycC\nVs+bpdCBN8sNE4OcWs8AA9Qag7BfkdsZzYvx81W8/DnuaA8Bsvt+GQbkPfCEz5hm\nX7f2Lc14FueKgUBcxwIDAQAB\n-----END PUBLIC KEY-----";

/// A key file with `text` in a directory of its own; returns its path.
fn key_file(name: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("datarig-mysql-key-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("key.pem");
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// A server packet: its length, sequence number `seq` and `payload`.
fn packet(seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![payload.len() as u8, (payload.len() >> 8) as u8, 0, seq];
    p.extend_from_slice(payload);
    p
}

/// The client's next packet (`None`: it closed the connection instead).
async fn read_packet(server: &mut tokio::io::DuplexStream) -> Option<Vec<u8>> {
    let mut header = [0u8; 4];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), server.read_exact(&mut header)).await;
    read.expect("the client neither answers nor closes").ok()?;
    let len = usize::from(header[0]) | usize::from(header[1]) << 8 | usize::from(header[2]) << 16;
    let mut payload = vec![0; len];
    server.read_exact(&mut payload).await.ok()?;
    Some(payload)
}

/// A `caching_sha2_password` login of `cfg` (over a stream of `route`'s kind, opened here) to
/// a mock server that answers its scramble with `answer` (`[1, 4]`: the full exchange, the
/// account is not in the server's cache; `[1, 3]`: a cached account). What the client sent
/// next (`None`: nothing, it closed), and how its login ended once the server stopped.
async fn login(cfg: &ConnectionConfig, route: &Route, answer: &[u8]) -> (Option<Vec<u8>>, Result<(), DbError>) {
    login_keyed(cfg, route, answer, None).await
}

/// [`login`], with the server answering the client's request for its key (`[2]`) with the
/// packet `key`; what the client sent after that.
async fn login_keyed(
    cfg: &ConnectionConfig,
    route: &Route,
    answer: &[u8],
    key: Option<&[u8]>,
) -> (Option<Vec<u8>>, Result<(), DbError>) {
    let opts = Target::of(cfg).unwrap().opts(None, "datarig-key-test", route);
    let (client, mut server) = tokio::io::duplex(1 << 16);
    let login = tokio::spawn(Conn::connect_with_stream(opts, Box::new(client)));
    server.write_all(&server_greeting()).await.unwrap();
    read_packet(&mut server).await.expect("the handshake response");
    server.write_all(&packet(2, answer)).await.unwrap();
    if answer == [1, 3] {
        // A cached account: OK, and the login is done.
        server.write_all(&packet(3, &[0, 0, 0, 2, 0, 0, 0])).await.unwrap();
    }
    let mut sent = match answer {
        [1, 4] => read_packet(&mut server).await,
        _ => None,
    };
    if let (Some(key), Some([2])) = (key, sent.as_deref()) {
        server.write_all(&packet(4, key)).await.unwrap();
        sent = read_packet(&mut server).await;
    }
    // A cached login ends here; any other ends with the stream.
    if answer != [1, 3] {
        drop(server);
    }
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), login).await.expect("the login ends");
    (sent, ended.unwrap().map(drop).map_err(|e| connect_error(&e).0))
}

fn mysql(host: &str) -> ConnectionConfig {
    ConnectionConfig {
        driver: "mysql".into(),
        host: host.into(),
        user: "u".into(),
        password: "s3cret-Pass-zz".into(),
        ..Default::default()
    }
}

/// Directly to another machine: the client never asks for the server's key (it would come
/// over the same unencrypted connection) and fails before anything about the password is
/// sent, unless the profile allows it.
#[tokio::test]
async fn a_direct_login_to_another_machine_never_asks_the_server_for_its_key() {
    let cfg = mysql("db.example.com");
    let route = Route::new(&cfg.host, 3306, None);
    let (sent, ended) = login(&cfg, &route, &[1, 4]).await;
    assert_eq!(sent, None, "nothing after the request for the full exchange");
    assert_eq!(ended, Err(DbError::KeyRetrievalRefused));
    // Not about the password: no prompt.
    let refused = mysql_async::Error::Driver(mysql_async::DriverError::PublicKeyRetrievalDisabled);
    assert_eq!(connect_error(&refused), (DbError::KeyRetrievalRefused, false));
    // The profile allows it.
    let allowed = ConnectionConfig { allow_public_key_retrieval: true, ..cfg };
    assert_eq!(login(&allowed, &route, &[1, 4]).await.0, Some(vec![2]));
}

/// A loopback server and one behind an SSH tunnel are still asked for their key.
#[tokio::test]
async fn loopback_and_tunnelled_logins_ask_the_server_for_its_key() {
    for host in ["localhost", "127.0.0.1", "::1"] {
        let cfg = mysql(host);
        let (sent, _) = login(&cfg, &Route::new(host, 3306, None), &[1, 4]).await;
        assert_eq!(sent, Some(vec![2]), "{host}");
    }
    let cfg = mysql("db.internal");
    let tunnel = Route::new(&cfg.host, 3306, Some(DialerRef(Arc::new(TcpDialer::default()))));
    assert_eq!(login(&cfg, &tunnel, &[1, 4]).await.0, Some(vec![2]));
}

/// With the profile's key file the password is encrypted with that key at once: the server
/// is never asked for one, on any route.
#[tokio::test]
async fn a_pinned_server_key_is_used_and_the_server_is_never_asked_for_one() {
    let path = key_file("pinned", KEY_2048);
    for host in ["db.example.com", "127.0.0.1"] {
        let cfg = ConnectionConfig { server_public_key_file: Some(path.clone()), ..mysql(host) };
        let (sent, _) = login(&cfg, &Route::new(host, 3306, None), &[1, 4]).await;
        let sent = sent.expect("the encrypted password");
        // RSA with a 2048-bit key: 256 bytes, and never the password itself.
        assert_eq!(sent.len(), 256, "{host}: {sent:?}");
        assert!(!sent.windows(14).any(|w| w == b"s3cret-Pass-zz"));
    }
}

/// RSA-OAEP with a 2048-bit key takes 214 bytes: the password and the zero byte the client
/// ends it with. A longer password is refused before anything about it is sent (mysql_common
/// would panic, and take the session's task with it), with the profile's key and with one the
/// server sends.
#[tokio::test]
async fn a_password_too_long_for_the_server_key_is_refused() {
    let path = key_file("long", KEY_2048);
    let pinned = |password: String| ConnectionConfig {
        server_public_key_file: Some(path.clone()),
        password,
        ..mysql("db.example.com")
    };
    let route = Route::new("db.example.com", 3306, None);
    let (sent, _) = login(&pinned("p".repeat(213)), &route, &[1, 4]).await;
    assert_eq!(sent.map(|s| s.len()), Some(256), "213 bytes fit");
    let (sent, ended) = login(&pinned("p".repeat(214)), &route, &[1, 4]).await;
    assert_eq!(sent, None);
    assert_eq!(ended, Err(DbError::PasswordTooLong { max: 213 }));
    // Bytes, not characters: 72 Hangul syllables are 216 bytes.
    let (sent, ended) = login(&pinned("\u{D55C}".repeat(72)), &route, &[1, 4]).await;
    assert_eq!((sent, ended), (None, Err(DbError::PasswordTooLong { max: 213 })));
    let mut key = vec![1];
    key.extend_from_slice(KEY_2048.as_bytes());
    let loopback = Route::new("127.0.0.1", 3306, None);
    let asked = |password: String| ConnectionConfig { password, ..mysql("127.0.0.1") };
    let (sent, _) = login_keyed(&asked("p".repeat(213)), &loopback, &[1, 4], Some(&key)).await;
    assert_eq!(sent.map(|s| s.len()), Some(256));
    let (sent, ended) = login_keyed(&asked("p".repeat(214)), &loopback, &[1, 4], Some(&key)).await;
    assert_eq!((sent, ended), (None, Err(DbError::PasswordTooLong { max: 213 })));
    let refused = mysql_async::Error::Driver(mysql_async::DriverError::PasswordTooLongForKey { max: 213 });
    assert_eq!(connect_error(&refused), (DbError::PasswordTooLong { max: 213 }, false));
}

/// A server that answers the request for its key without one ends the login with an error.
#[tokio::test]
async fn an_empty_key_packet_from_the_server_is_an_error() {
    let cfg = mysql("127.0.0.1");
    let route = Route::new("127.0.0.1", 3306, None);
    for key in [&[][..], &[1][..]] {
        let (sent, ended) = login_keyed(&cfg, &route, &[1, 4], Some(key)).await;
        assert_eq!(sent, None, "{key:?}");
        assert!(matches!(ended, Err(DbError::Connection(_))), "{key:?}: {ended:?}");
    }
}

/// A key the server sends is checked as the profile's key file is (an RSA public key in PEM
/// of 2048 bits or more) before the password is encrypted with it: anything else ends the
/// login with an error (mysql_common would panic on a malformed one), and nothing about the
/// password is sent.
#[tokio::test]
async fn a_malformed_key_from_the_server_is_an_error() {
    let cfg = mysql("127.0.0.1");
    let route = Route::new("127.0.0.1", 3306, None);
    let bad = [
        "garbage".to_string(),
        KEY_1024.into(),
        KEY_2048.replace("MIIBIjAN", "MIIBIjAn"),
        KEY_2048.replace("twIDAQAB", "twIDAQA"),
        KEY_2048.replace("-----END PUBLIC KEY-----", ""),
        "-----BEGIN PUBLIC KEY-----\nMAA=\n-----END PUBLIC KEY-----".into(),
    ];
    for (i, text) in bad.iter().enumerate() {
        let mut key = vec![1];
        key.extend_from_slice(text.as_bytes());
        let (sent, ended) = login_keyed(&cfg, &route, &[1, 4], Some(&key)).await;
        assert_eq!((sent, ended), (None, Err(DbError::ServerKeyInvalid)), "{i}");
    }
    let invalid = mysql_async::Error::Driver(mysql_async::DriverError::InvalidServerPublicKey);
    assert_eq!(connect_error(&invalid), (DbError::ServerKeyInvalid, false));
}

/// A relative key file is refused when the session opens (it would be read from wherever
/// datarig was started); `~/` is the home directory.
#[test]
fn a_relative_server_key_file_is_refused() {
    let target = |path: &str| Target::of(&ConnectionConfig { server_public_key_file: Some(path.into()), ..mysql("h") });
    for path in ["keys/x.pem", "key.pem", "./key.pem", "~key.pem"] {
        assert_eq!(target(path).err(), Some(DbError::ServerKeyPathRelative(path.into())), "{path}");
    }
    assert!(matches!(target("~/datarig-no-such-key.pem"), Err(DbError::ServerKeyFile { fault: Some(_), .. })));
}

/// A cached account logs in without any key, everywhere.
#[tokio::test]
async fn a_cached_login_needs_no_key() {
    let cfg = mysql("db.example.com");
    let (_, ended) = login(&cfg, &Route::new(&cfg.host, 3306, None), &[1, 3]).await;
    assert_eq!(ended, Ok(()));
}

/// The key file is read when the session opens: one that cannot be read, or that is not an
/// RSA public key of 2048 bits or more, is a settings error that names it.
#[test]
fn the_server_key_file_must_be_an_rsa_public_key() {
    let target = |path: &str| Target::of(&ConnectionConfig { server_public_key_file: Some(path.into()), ..mysql("h") });
    assert!(target(&key_file("pkcs8", KEY_2048)).is_ok());
    assert!(target(&key_file("pkcs1", KEY_2048_PKCS1)).is_ok());
    let missing = key_file("missing", KEY_2048).replace("key.pem", "none.pem");
    assert!(
        matches!(target(&missing), Err(DbError::ServerKeyFile { ref path, fault: Some(_) }) if *path == missing),
        "{:?}",
        target(&missing).err()
    );
    let not_a_key = [
        String::new(),
        "hello".into(),
        KEY_1024.into(),
        KEY_2048.replace("MIIBIjAN", "MIIBIjAn"),
        KEY_2048.replace("twIDAQAB", "twIDAQA"),
        KEY_2048.replace("-----END PUBLIC KEY-----", ""),
        "-----BEGIN PUBLIC KEY-----\nMAA=\n-----END PUBLIC KEY-----".into(),
        "-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----".into(),
    ];
    for (i, text) in not_a_key.iter().enumerate() {
        let path = key_file(&format!("bad{i}"), text);
        assert!(
            matches!(target(&path), Err(DbError::ServerKeyFile { fault: None, .. })),
            "{i}: {:?}",
            target(&path).err()
        );
    }
}

/// The session's values are known once the server reported its time zone and both character
/// sets; the one that is not `utf8mb4` is the one that matters.
#[test]
fn session_values_are_known_only_as_reported() {
    let mut t = Tracked::default();
    assert_eq!(t.values(), None, "nothing reported: not known");
    t.time_zone = Some("SYSTEM".into());
    t.results_charset = Some("utf8mb4".into());
    assert_eq!(t.values(), None, "the connection's character set not reported");
    t.connection_charset = Some("utf8mb4".into());
    let v = |t: &Tracked| t.values().map(|v| (v.time_zone, v.charset));
    assert_eq!(v(&t), Some(("SYSTEM".into(), "utf8mb4".into())));
    t.connection_charset = Some("latin1".into());
    assert_eq!(v(&t), Some(("SYSTEM".into(), "latin1".into())));
    t.results_charset = Some("utf8mb3".into());
    assert_eq!(v(&t), Some(("SYSTEM".into(), "utf8mb3".into())));
    t.results_charset = Some("NULL".into());
    assert_eq!(v(&t), Some(("SYSTEM".into(), "binary".into())));
}

/// What the session asks after server code ran replaces what it had; a failed question leaves
/// the values unknown (no fixed zone, no UTF-8), never the old ones.
#[test]
fn asked_values_replace_the_old_and_a_failed_question_forgets_them() {
    let mut t = Tracked::default();
    t.set_asked(("NO_BACKSLASH_ESCAPES".into(), "+09:00".into(), "latin1".into(), "utf8mb4".into()));
    assert_eq!(t.sql_mode.as_deref(), Some("NO_BACKSLASH_ESCAPES"));
    assert_eq!(t.values().map(|v| (v.time_zone, v.charset)), Some(("+09:00".into(), "latin1".into())));
    t.forget_values();
    assert_eq!(t.values().map(|v| (v.time_zone, v.charset)), Some(("unknown".into(), "unknown".into())));
}

/// A killer of connection 7 over `route` (nothing is sent before it is asked to).
fn killer_over(route: Route) -> Killer {
    let opts = mysql_async::Opts::from_url("mysql://datarig@127.0.0.1:1/").unwrap();
    Killer::new(route, opts, 7)
}

#[tokio::test]
async fn a_kill_that_cannot_reach_the_server_is_not_sent() {
    // Nothing listens on port 1: the connection to send the kill on does not open.
    let k = killer_over(Route::new("127.0.0.1", 1, None));
    k.kill_query().await;
    assert_eq!(*k.watch().borrow(), Kill::Failed(KillFailure::NotSent));
    // One that was for a statement that ended meanwhile says nothing.
    k.clear();
    k.kill_query_if(|| false).await;
    assert_eq!(*k.watch().borrow(), Kill::None);
}

#[test]
fn a_kill_of_a_run_before_never_reaches_the_next_one() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let k = killer_over(Route::new("127.0.0.1", 1, None));
    let asked = Arc::new(AtomicBool::new(true));
    let still = {
        let asked = asked.clone();
        move || asked.load(Ordering::SeqCst)
    };
    let refused = Kill::Failed(KillFailure::Refused(1095));
    // The kill of the run that is still on is kept.
    k.record(refused, &still);
    assert_eq!(*k.watch().borrow(), refused);
    // A new run begins (the request is forgotten, then the kills): what the old one's kill
    // answers after that is not kept, whenever it lands.
    asked.store(false, Ordering::SeqCst);
    k.record(refused, &still);
    k.clear();
    k.record(Kill::Taken(Instant::now()), &still);
    assert_eq!(*k.watch().borrow(), Kill::None);
    // The new run's own cancel is.
    asked.store(true, Ordering::SeqCst);
    k.record(refused, &still);
    assert_eq!(*k.watch().borrow(), refused);
}

/// A dialer that hands out the client's end of an in-memory stream, once.
struct OneStream(std::sync::Mutex<Option<tokio::io::DuplexStream>>);

impl datarig_core::transport::Dialer for OneStream {
    fn dial(
        &self,
        _host: &str,
        _port: u16,
    ) -> futures::future::BoxFuture<
        'static,
        Result<datarig_core::transport::BoxedStream, datarig_core::transport::DialError>,
    > {
        let s = self.0.lock().unwrap().take();
        Box::pin(async move {
            s.map(|s| Box::new(s) as datarig_core::transport::BoxedStream)
                .ok_or(datarig_core::transport::DialError::NotOpen)
        })
    }
}

/// A killer of connection 7 whose connection is the far end (the server's) of the result.
fn killer_on_a_fake_server() -> (Killer, tokio::io::DuplexStream) {
    let (client, server) = tokio::io::duplex(1 << 16);
    let cfg = mysql("127.0.0.1");
    let route = Route::new(&cfg.host, 3306, Some(DialerRef(Arc::new(OneStream(std::sync::Mutex::new(Some(client)))))));
    let opts = Target::of(&cfg).unwrap().opts(None, "datarig-k-test", &route);
    (Killer::new(route, opts, 7), server)
}

/// Up to the client's login answer (the handshake response read, fast authentication next).
async fn greet(server: &mut tokio::io::DuplexStream) {
    server.write_all(&server_greeting()).await.unwrap();
    read_packet(server).await.expect("the handshake response");
}

/// Fast authentication done, the login OK.
async fn logged_in(server: &mut tokio::io::DuplexStream) {
    server.write_all(&packet(2, &[1, 3])).await.unwrap();
    server.write_all(&packet(3, &[0, 0, 0, 2, 0, 0, 0])).await.unwrap();
}

/// A kill sent again whose statement answered while its connection opened is never sent: the
/// next packet the server gets is the client's goodbye, never `KILL QUERY`.
#[tokio::test]
async fn a_kill_sent_again_after_its_statement_answered_is_never_sent() {
    let (killer, mut server) = killer_on_a_fake_server();
    let gate = Gate::new();
    killer.try_again(&gate);
    greet(&mut server).await;
    // The statement answers while the kill's login goes on.
    gate.close().await;
    logged_in(&mut server).await;
    let next = read_packet(&mut server).await;
    assert!(
        next.as_deref().is_none_or(|p| p.first() != Some(&3)),
        "{:?}",
        next.map(|p| String::from_utf8_lossy(&p).into_owned())
    );
    assert_eq!(*killer.watch().borrow(), Kill::None);
}

/// Closing the gate waits for a kill being sent to be answered: none sent while the statement
/// ran can land on the session's next statement.
#[tokio::test]
async fn closing_the_gate_waits_for_a_kill_being_sent() {
    let (killer, mut server) = killer_on_a_fake_server();
    let gate = Gate::new();
    killer.try_again(&gate);
    greet(&mut server).await;
    logged_in(&mut server).await;
    let sent = read_packet(&mut server).await.expect("the kill");
    assert_eq!(&sent[..], b"\x03KILL QUERY 7");
    let closing = tokio::spawn({
        let gate = gate.clone();
        async move { gate.close().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!closing.is_finished(), "waits for the kill's answer");
    server.write_all(&packet(1, &[0, 0, 0, 2, 0, 0, 0])).await.unwrap();
    let clean = tokio::time::timeout(std::time::Duration::from_secs(2), closing).await.unwrap().unwrap();
    assert!(clean, "the kill was answered");
    assert!(matches!(*killer.watch().borrow(), Kill::Taken(_)), "taken while the gate was open");
}

/// With no kill being sent, closing the gate waits for nothing.
#[tokio::test]
async fn closing_a_gate_with_no_kill_on_its_way_is_free() {
    let gate = Gate::new();
    let t0 = Instant::now();
    assert!(gate.close().await);
    assert!(t0.elapsed() < std::time::Duration::from_millis(100), "{:?}", t0.elapsed());
}

/// A kill whose answer the server holds back past a couple of seconds is still waited for:
/// the session's next statement goes only once it is answered.
#[tokio::test]
async fn closing_the_gate_waits_for_a_kill_answered_late() {
    let (killer, mut server) = killer_on_a_fake_server();
    let gate = Gate::new();
    killer.try_again(&gate);
    greet(&mut server).await;
    logged_in(&mut server).await;
    read_packet(&mut server).await.expect("the kill");
    let t0 = Instant::now();
    let closing = tokio::spawn({
        let gate = gate.clone();
        async move { gate.close().await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(!closing.is_finished(), "still waits after 3 s");
    server.write_all(&packet(1, &[0, 0, 0, 2, 0, 0, 0])).await.unwrap();
    assert!(closing.await.unwrap(), "answered");
    assert!(t0.elapsed() >= std::time::Duration::from_secs(3));
}

/// A kill the server never answers: closing gives up once a kill may have taken its time
/// ([`KILL_TIMEOUT`]) and says so: the kill may still be carried out later, on whatever the
/// connection runs then.
#[tokio::test]
async fn a_kill_never_answered_leaves_the_gate_unclean() {
    let (killer, mut server) = killer_on_a_fake_server();
    let gate = Gate::new();
    killer.try_again(&gate);
    greet(&mut server).await;
    logged_in(&mut server).await;
    read_packet(&mut server).await.expect("the kill");
    let t0 = Instant::now();
    let clean = tokio::time::timeout(KILL_TIMEOUT + std::time::Duration::from_secs(2), gate.close()).await;
    assert_eq!(clean, Ok(false), "{:?}", t0.elapsed());
    assert!(t0.elapsed() < KILL_TIMEOUT + std::time::Duration::from_secs(1), "{:?}", t0.elapsed());
    drop(server);
}

/// A dialer that hands out the client's end of an in-memory stream once, and says when.
struct OneStreamSaid(std::sync::Mutex<Option<tokio::io::DuplexStream>>, Arc<tokio::sync::Notify>);

impl datarig_core::transport::Dialer for OneStreamSaid {
    fn dial(
        &self,
        _host: &str,
        _port: u16,
    ) -> futures::future::BoxFuture<
        'static,
        Result<datarig_core::transport::BoxedStream, datarig_core::transport::DialError>,
    > {
        let s = self.0.lock().unwrap().take();
        self.1.notify_one();
        Box::pin(async move {
            s.map(|s| Box::new(s) as datarig_core::transport::BoxedStream)
                .ok_or(datarig_core::transport::DialError::NotOpen)
        })
    }
}

/// A statement that answers while a kill sent again for it never gets its answer: what the
/// statement answered is the run's (an autocommitted write did commit) and comes back as it
/// is; the connection then runs nothing more, and the session is lost at its next command,
/// saying why.
#[tokio::test]
async fn a_statement_answered_while_its_kill_hangs_is_answered_then_lost() {
    use crate::link::{Link, Next};
    let (client, mut server) = tokio::io::duplex(1 << 16);
    let dialed = Arc::new(tokio::sync::Notify::new());
    let cfg = mysql("127.0.0.1");
    let dialer = OneStreamSaid(std::sync::Mutex::new(Some(client)), dialed.clone());
    let route = Route::new(&cfg.host, 3306, Some(DialerRef(Arc::new(dialer))));
    let opts = Target::of(&cfg).unwrap().opts(None, "datarig-k-test", &route);
    let killer = Killer::new(route, opts, 7);
    killer.record(Kill::Failed(KillFailure::Refused(1095)), || true);
    // The kill's server: the kill sent again (once the session asks again) is read, and never
    // answered.
    let written = Arc::new(tokio::sync::Notify::new());
    let kill_server = tokio::spawn({
        let written = written.clone();
        async move {
            dialed.notified().await;
            greet(&mut server).await;
            logged_in(&mut server).await;
            let kill = read_packet(&mut server).await;
            written.notify_one();
            tokio::time::sleep(KILL_TIMEOUT * 2).await;
            kill
        }
    });
    let (client, _session_server) = tokio::io::duplex(1 << 16);
    let (wire, mut _stream) = crate::wire::Wire::new(Box::new(client));
    let (_commands, rx) = tokio::sync::mpsc::unbounded_channel();
    let (events, _events) = tokio::sync::mpsc::unbounded_channel();
    let mut link = Link::new(rx, wire);
    link.serving(&events, 1);
    // The statement answers once the kill is on its way.
    let answered = link.guard(Some(&killer), async {
        written.notified().await;
        42
    });
    let answered = tokio::time::timeout(KILL_TIMEOUT * 3, answered).await.expect("the wait ends");
    assert!(matches!(answered, Ok(42)), "the statement's own answer");
    let next = tokio::time::timeout(std::time::Duration::from_secs(1), link.next()).await.expect("at once");
    assert!(matches!(next, Next::Lost(DbError::CancelNotTaken(CancelFailure::StopPending(KILL_TIMEOUT)))));
    assert!(_stream.write_all(b"next").await.is_err(), "the connection is cut");
    kill_server.abort();
}

/// A kill that could not be sent keeps why (here the server's port refuses the connection), so
/// the session that gives up on it says why, not just that it could not reach the server.
#[tokio::test]
async fn a_kill_not_sent_keeps_why() {
    let killer = Killer::new(
        Route::new("127.0.0.1", 1, None),
        mysql_async::Opts::from_url("mysql://datarig@127.0.0.1:1/").unwrap(),
        7,
    );
    killer.kill_query().await;
    assert_eq!(*killer.watch().borrow(), Kill::Failed(KillFailure::NotSent));
    match killer.not_sent() {
        datarig_core::driver::CancelFailure::NotSent(why) => {
            assert!(matches!(*why, DbError::Connection(_) | DbError::Transport(_)), "{why:?}");
        }
        other => panic!("{other:?}"),
    }
}

/// A column definition of a text column `name` (protocol 41).
fn column(name: &str) -> Vec<u8> {
    let mut p = Vec::new();
    for s in ["def", "shop", "t", "t", name, name] {
        p.push(s.len() as u8);
        p.extend_from_slice(s.as_bytes());
    }
    p.push(0x0c);
    p.extend_from_slice(&255u16.to_le_bytes());
    p.extend_from_slice(&1020u32.to_le_bytes());
    p.push(0xfd);
    p.extend_from_slice(&[0, 0, 0, 0, 0]);
    p
}

/// A procedure's answer that comes in several packets: a result set's head (one column), then
/// its `rows` and the `OK` that ends the call.
fn call_answer(rows: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let mut packets = vec![vec![1], column("v")];
    for r in rows {
        let mut row = vec![r.len() as u8];
        row.extend_from_slice(r.as_bytes());
        packets.push(row);
    }
    // The result set's end (`OK` in place of `EOF`), more results to come; then the call's.
    packets.push(vec![0xfe, 0, 0, 0x0a, 0, 0, 0]);
    packets.push(vec![0, 0, 0, 0x02, 0, 0, 0]);
    let mut wire = packets.iter().zip(1u8..).map(|(p, seq)| packet(seq, p));
    let head = wire.by_ref().take(2).flatten().collect();
    (head, wire.flatten().collect())
}

/// A procedure whose result set comes in several packets answers while a kill sent again for it
/// never gets its answer, its rows only after the session gave up on that answer: its rows and
/// its end are read and are the run's, never cut short; no
/// request follows it on the connection, and the session is lost at its next command, saying
/// that the kill was sent and not answered.
#[tokio::test]
async fn a_result_answered_while_its_kill_hangs_is_read_to_its_end_then_the_session_is_lost() {
    use datarig_core::driver::{DbCommand, DbEvent, PagingMode};
    // The kill's server: the kill sent again is read, and never answered.
    let (kill_client, mut kill_server) = tokio::io::duplex(1 << 16);
    let dialed = Arc::new(tokio::sync::Notify::new());
    let cfg = mysql("127.0.0.1");
    let dialer = OneStreamSaid(std::sync::Mutex::new(Some(kill_client)), dialed.clone());
    let route = Route::new(&cfg.host, 3306, Some(DialerRef(Arc::new(dialer))));
    let opts = Target::of(&cfg).unwrap().opts(None, "datarig-q-test", &route);
    let killer = Killer::new(route, opts.clone(), 7);
    let written = Arc::new(tokio::sync::Notify::new());
    let kills = tokio::spawn({
        let written = written.clone();
        async move {
            dialed.notified().await;
            greet(&mut kill_server).await;
            logged_in(&mut kill_server).await;
            let kill = read_packet(&mut kill_server).await;
            written.notify_one();
            tokio::time::sleep(KILL_TIMEOUT * 3).await;
            kill
        }
    });
    // The session's own connection.
    let (client, mut server) = tokio::io::duplex(1 << 16);
    let (wire, stream) = Wire::new(Box::new(client));
    let login = tokio::spawn(Conn::connect_with_stream(opts, Box::new(stream)));
    greet(&mut server).await;
    logged_in(&mut server).await;
    let conn = login.await.unwrap().unwrap();
    let (commands, rx) = tokio::sync::mpsc::unbounded_channel();
    let (events, mut got) = tokio::sync::mpsc::unbounded_channel();
    let session = tokio::spawn(crate::query::query_loop(
        conn,
        crate::link::Link::new(rx, wire),
        killer.clone(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
        events,
        crate::query::Settings { page_size: 100, read_only: false },
        MYSQL_84,
        Tracked::default(),
        None,
    ));
    let call = "CALL shop.zz_rows()";
    commands.send(DbCommand::Execute { id: 1, statements: vec![call.into()], paging: PagingMode::NoHold }).unwrap();
    let sent = read_packet(&mut server).await.expect("the call");
    assert_eq!(&sent[1..], call.as_bytes());
    // The server refused the user's cancel (the call runs as its definer); the session sends it
    // again, and the call answers once that kill is on its way.
    killer.record(Kill::Failed(KillFailure::Refused(KILL_DENIED)), || true);
    tokio::time::timeout(std::time::Duration::from_secs(30), written.notified()).await.expect("the kill sent again");
    // Its head comes first; the rest only once the session gave up on the kill's answer.
    let (head, tail) = call_answer(&["a", "b", "c"]);
    server.write_all(&head).await.unwrap();
    tokio::time::sleep(KILL_TIMEOUT + std::time::Duration::from_secs(2)).await;
    // (A session that cut its connection reads none of it.)
    let _ = server.write_all(&tail).await;
    // No request follows the call: the connection is closed instead.
    let after = tokio::time::timeout(KILL_TIMEOUT * 2, async {
        let mut buf = [0u8; 64];
        server.read(&mut buf).await.map(|n| buf[..n].to_vec())
    })
    .await
    .expect("the connection ends");
    assert!(after.as_ref().is_ok_and(Vec::is_empty) || after.is_err(), "nothing sent after the call: {after:?}");
    tokio::time::timeout(std::time::Duration::from_secs(5), session).await.expect("the session ends").unwrap();
    let evs: Vec<DbEvent> = std::iter::from_fn(|| got.try_recv().ok()).collect();
    let page = evs.iter().find_map(|e| match e {
        DbEvent::Page { id: 1, columns: Some(_), rows, more, .. } => Some((rows.clone(), *more)),
        _ => None,
    });
    let rows = page.unwrap_or_else(|| panic!("the call's rows: {evs:?}"));
    let want: Vec<Vec<Option<String>>> = ["a", "b", "c"].iter().map(|v| vec![Some(v.to_string())]).collect();
    assert_eq!(rows.0, want, "{evs:?}");
    assert!(!rows.1, "the result ended");
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Failed { .. })), "{evs:?}");
    match evs.last() {
        Some(DbEvent::Lost { error: DbError::CancelNotTaken(CancelFailure::StopPending(KILL_TIMEOUT)) }) => {}
        other => panic!("{other:?}"),
    }
    kills.abort();
}

/// A kill written to the server that never gets its answer was sent: it is not one that could
/// not reach the server (it may still be carried out), and is said so.
#[tokio::test]
async fn a_kill_sent_and_never_answered_is_not_taken_for_one_not_sent() {
    let (killer, mut server) = killer_on_a_fake_server();
    let kill_server = tokio::spawn(async move {
        greet(&mut server).await;
        logged_in(&mut server).await;
        let kill = read_packet(&mut server).await;
        // Held open past the time the killer waits for the answer.
        tokio::time::sleep(KILL_TIMEOUT + std::time::Duration::from_secs(1)).await;
        kill
    });
    killer.kill_query().await;
    assert_eq!(*killer.watch().borrow(), Kill::Failed(KillFailure::NotAnswered));
    assert!(kill_server.await.unwrap().is_some_and(|p| p == b"\x03KILL QUERY 7"), "it was sent");
}
