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
