//! TLS in the UI: what a connection says about its encryption (status bar, connection bar, the
//! profile form's test), and the profile form's TLS settings.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbError, DbEvent, PingInfo};
use datarig_core::i18n::Lang;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::PasswordSource;
use datarig_core::tls::{Encryption, FileProblem, TlsFile, TlsMode, TlsProblem, TlsVersion, Verified};
use datarig_tui::app::profiles::{Field, FieldError, Section};
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

/// PostgreSQL profiles `db-remote` (another machine) and `lab-local` (this one), launched with
/// the explorer's cursor on the first; nothing connects until a test says so.
fn launched() -> Harness {
    let profile = |name: &str, host: &str| {
        let mut p = ConnectionConfig {
            name: name.into(),
            host: host.into(),
            user: "app".into(),
            database: "app".into(),
            ..ConnectionConfig::default()
        };
        p.set_source(PasswordSource::File);
        p
    };
    let cfg = Config {
        connections: vec![profile("db-remote", "db.example.com"), profile("lab-local", "127.0.0.1")],
        ..Config::default()
    };
    let store = std::sync::Arc::new(datarig_core::secret::MemoryStore::new());
    Harness::launched(&cfg, Lang::En, store, datarig_tui::app::Startup::Normal).with_fake_driver()
}

const TLS13: TlsVersion = TlsVersion::Tls13;

fn connect(h: &mut Harness, name: &str, encryption: Option<Encryption>) {
    h.command(&format!("conn {name}"));
    if let Some(e) = encryption {
        h.meta_db(name, DbEvent::Encryption(e));
    }
    h.meta_db(name, DbEvent::Connected);
}

#[test]
fn a_connection_says_how_it_is_encrypted() {
    let mut h = launched();
    connect(&mut h, "db-remote", Some(Encryption::Tls { version: TLS13, verified: Verified::Full }));
    let status = h.status(200, 40);
    assert!(status.contains("Connected to db-remote · TLS 1.3, server verified"), "{status}");
    h.ctrl('t');
    let screen = h.screen(160, 40);
    let bar = screen.lines().find(|l| l.contains("db.example.com:5432")).unwrap_or_default().to_string();
    assert!(bar.contains("· TLS 1.3") && !bar.contains("not verified"), "{screen}");
    // Not checked (prefer, require): said so.
    connect(&mut h, "db-remote", Some(Encryption::Tls { version: TlsVersion::Tls12, verified: Verified::No }));
    let status = h.status(200, 40);
    assert!(status.contains("TLS 1.2, certificate not checked"), "{status}");
    let screen = h.screen(160, 40);
    assert!(screen.contains("TLS 1.2 · not verified"), "{screen}");
    // The chain only (verify-ca).
    connect(&mut h, "db-remote", Some(Encryption::Tls { version: TLS13, verified: Verified::Chain }));
    assert!(h.status(200, 40).contains("TLS 1.3, CA checked (not the host name)"));
    assert!(h.screen(160, 40).contains("TLS 1.3 · CA only"));
}

#[test]
fn a_plain_connection_to_another_machine_is_a_warning_and_never_silent() {
    let mut h = launched();
    // `prefer` that got a plain connection (the server offered no TLS).
    connect(&mut h, "db-remote", Some(Encryption::Plain));
    let status = h.status(200, 40);
    assert!(status.contains("Connected to db-remote without encryption"), "{status}");
    assert!(status.contains("set the profile's SSL mode to require or stricter, or use an SSH tunnel"), "{status}");
    h.ctrl('t');
    assert!(h.screen(160, 40).contains("· unencrypted"), "{}", h.screen(160, 40));
    // To this machine: plain is nothing to warn about, and the bar says nothing.
    connect(&mut h, "lab-local", Some(Encryption::Plain));
    let status = h.status(200, 40);
    assert!(status.contains("Connected to lab-local") && !status.contains("without encryption"), "{status}");
    h.ctrl('t');
    let screen = h.screen(160, 40);
    let bar = screen.lines().find(|l| l.contains("127.0.0.1:5432")).unwrap_or_default().to_string();
    assert!(!bar.contains("unencrypted") && !bar.contains("TLS"), "{screen}");
}

#[test]
fn an_earlier_attempts_encryption_is_not_kept() {
    let mut h = launched();
    connect(&mut h, "db-remote", Some(Encryption::Tls { version: TLS13, verified: Verified::Full }));
    let id = h.app.profiles[0].id;
    assert!(h.app.conns.get(id).unwrap().encryption.is_some());
    // A new attempt starts knowing nothing of it (until its session says).
    h.app.conns.next_generation(id);
    assert_eq!(h.app.conns.get(id).unwrap().encryption, None);
}

#[test]
fn the_form_test_says_how_the_test_connection_was_encrypted() {
    let mut h = launched();
    h.command("conn.edit");
    h.ctrl('t');
    let ok = |encryption| Ok(PingInfo { server_version: "17.2".into(), latency: Duration::from_millis(9), encryption });
    h.ping(ok(Encryption::Tls { version: TLS13, verified: Verified::Full }));
    let screen = h.screen(160, 45);
    assert!(screen.contains("Connection OK · server 17.2 · 9ms · TLS 1.3, server verified"), "{screen}");
    h.ctrl('t');
    h.ping(ok(Encryption::Plain));
    assert!(h.screen(160, 45).contains("Connection OK · server 17.2 · 9ms · unencrypted"));
}

/// The words of a failed attempt of `db-remote` with `error` (the status bar's notice, whole).
fn failure(h: &mut Harness, error: DbError) -> String {
    h.command("conn db-remote");
    h.meta_db("db-remote", DbEvent::ConnectFailed { error, auth: false });
    let notice = h.app.status.clone().expect("a notice");
    h.app.i18n.msg(&notice.msg).to_string()
}

#[test]
fn tls_failures_read_as_what_went_wrong() {
    let mut h = launched();
    let cases = [
        (TlsProblem::Unsupported, "the server does not offer TLS"),
        (TlsProblem::Untrusted, "the server's certificate is not trusted"),
        (TlsProblem::NameMismatch { host: "db.example.com".into() }, "is not for db.example.com"),
        (TlsProblem::Expired, "has expired"),
        (TlsProblem::NotYetValid, "is not valid yet"),
        (TlsProblem::Revoked, "was revoked"),
        (TlsProblem::ClientCertRejected, "refused the client certificate"),
        (TlsProblem::NeedsCaFile, "against a CA file, and the profile names none"),
        (TlsProblem::CertWithoutKey, "needs its key file"),
        (TlsProblem::UnknownMode("verify".into()), "“verify” is not one this driver knows"),
        (
            TlsProblem::File { file: TlsFile::Key, path: "/k.pem".into(), problem: FileProblem::Encrypted },
            "/k.pem is protected by a passphrase",
        ),
        (
            TlsProblem::File { file: TlsFile::Ca, path: "/ca.pem".into(), problem: FileProblem::NoCertificate },
            "the CA file /ca.pem has no certificate",
        ),
        (TlsProblem::PathRelative { file: TlsFile::Cert, path: "c.pem".into() }, "client certificate file c.pem must"),
        (TlsProblem::PasswordUnverified, "whose certificate was not checked"),
    ];
    for (problem, words) in cases {
        let t = failure(&mut h, DbError::Tls(problem.clone()));
        assert!(t.contains(words), "{problem:?}: {t}");
    }
    // The one a MySQL server sends when it requires TLS and the profile's mode is off.
    let t = failure(&mut h, DbError::TlsRequired);
    assert!(t.contains("set it to PREFERRED or stricter"), "{t}");
}

#[test]
fn the_form_edits_and_checks_the_tls_settings() {
    let mut h = launched();
    h.command("conn.edit");
    h.ctrl('p');
    assert_eq!((h.form().section, h.form().focus), (Section::Advanced, Field::SslMode));
    // No mode in the file: the one it connects with (disable for PostgreSQL).
    assert_eq!(h.form().tls_mode(), TlsMode::Disable);
    let errors = |h: &mut Harness| h.form().errors(|_| false);
    // verify-ca without a CA file.
    h.app.overlays.form_mut().unwrap().pick(Field::SslMode, TlsMode::VerifyCa.index());
    assert!(errors(&mut h).contains(&(Field::TlsCa, FieldError::NeedsCaFile)));
    assert!(h.screen(160, 45).contains("needed for this SSL mode"), "{}", h.screen(160, 45));
    h.key(KeyCode::Tab);
    h.type_text("certs/ca.pem");
    assert!(errors(&mut h).contains(&(Field::TlsCa, FieldError::AbsolutePath)));
    h.ctrl('u');
    h.type_text("/etc/ssl/db/ca.pem");
    assert!(errors(&mut h).is_empty(), "{:?}", errors(&mut h));
    // A client certificate without its key, then with it.
    h.key(KeyCode::Tab);
    h.type_text("~/certs/me.pem");
    assert!(errors(&mut h).contains(&(Field::TlsKey, FieldError::CertNeedsKey)));
    h.key(KeyCode::Tab);
    h.type_text("~/certs/me.key");
    assert!(errors(&mut h).is_empty());
    // The URL follows.
    let dsn = h.form().dsn.text().to_string();
    assert!(
        dsn.ends_with("?sslmode=verify-ca&sslrootcert=%2Fetc%2Fssl%2Fdb%2Fca.pem&sslcert=~%2Fcerts%2Fme.pem&sslkey=~%2Fcerts%2Fme.key"),
        "{dsn}"
    );
    h.ctrl('s');
    let p = h.app.profiles.iter().find(|p| p.name == "db-remote").unwrap();
    assert_eq!(
        (p.sslmode.as_str(), p.sslrootcert.as_deref(), p.sslcert.as_deref(), p.sslkey.as_deref()),
        ("verify-ca", Some("/etc/ssl/db/ca.pem"), Some("~/certs/me.pem"), Some("~/certs/me.key"))
    );
    // With TLS off, the files may stay (unused), and nothing about them is required.
    h.command("conn.edit");
    h.app.overlays.form_mut().unwrap().pick(Field::SslMode, TlsMode::Disable.index());
    h.app.overlays.form_mut().unwrap().focus_field(Field::TlsKey);
    h.ctrl('u');
    assert!(errors(&mut h).is_empty(), "{:?}", errors(&mut h));
}

#[test]
fn a_pasted_url_with_tls_parameters_fills_the_tls_fields() {
    let mut h = launched();
    // Pasted on the explorer: a new profile of the URL.
    h.app.handle_event(ratatui::crossterm::event::Event::Paste(
        "postgres://me@db.example.com/app?sslmode=verify-full&sslrootcert=/etc/ca.pem".into(),
    ));
    let f = h.form();
    assert_eq!((f.tls_mode(), f.tls_ca.text()), (TlsMode::VerifyFull, "/etc/ca.pem"));
    assert_eq!(f.dsn_problem, None);
    // Another parameter is still refused.
    h.key(KeyCode::Esc);
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("postgres://h/db?connect_timeout=3".into()));
    let problem = h.form().dsn_problem.clone().map(|p| h.app.i18n.msg(&p.message()).to_string());
    assert_eq!(
        problem.as_deref(),
        Some("unsupported parameter “connect_timeout” (only sslmode, sslrootcert, sslcert, sslkey)")
    );
}

#[test]
fn a_mysql_profile_picks_its_mode_by_mysqls_names() {
    let mut cfg = Config::default();
    let mut my = ConnectionConfig {
        name: "shop".into(),
        driver: "mysql".into(),
        host: "db.example.com".into(),
        port: 3306,
        user: "app".into(),
        // What the form wrote into MySQL profiles before they had TLS: never read.
        sslmode: "disable".into(),
        ..ConnectionConfig::default()
    };
    my.set_source(PasswordSource::File);
    cfg.connections.push(my);
    let store = std::sync::Arc::new(datarig_core::secret::MemoryStore::new());
    let mut h = Harness::launched(&cfg, Lang::En, store, datarig_tui::app::Startup::Normal).with_fake_driver();
    h.command("conn.edit");
    h.ctrl('p');
    assert_eq!(h.form().tls_mode(), TlsMode::Prefer, "the old value is no mode: PREFERRED");
    h.key(KeyCode::Enter);
    let screen = h.screen(120, 40);
    for name in ["DISABLED", "PREFERRED", "REQUIRED", "VERIFY_CA", "VERIFY_IDENTITY"] {
        assert!(screen.contains(name), "{name}: {screen}");
    }
    h.key(KeyCode::Esc);
    h.key(KeyCode::Right);
    h.ctrl('s');
    assert_eq!(h.app.profiles[0].sslmode, "REQUIRED");
}

#[test]
fn an_unknown_mode_is_shown_as_an_error_until_one_is_picked() {
    let mut cfg = Config::default();
    let mut p =
        ConnectionConfig { name: "x".into(), host: "h".into(), user: "u".into(), ..ConnectionConfig::default() };
    p.sslmode = "verify".into();
    p.set_source(PasswordSource::File);
    cfg.connections.push(p);
    let store = std::sync::Arc::new(datarig_core::secret::MemoryStore::new());
    let mut h = Harness::launched(&cfg, Lang::En, store, datarig_tui::app::Startup::Normal).with_fake_driver();
    h.command("conn.edit");
    assert!(h.form().errors(|_| false).contains(&(Field::SslMode, FieldError::UnknownMode)));
    // Saving would replace it: refused while it shows.
    h.ctrl('s');
    assert!(h.form_open());
    assert_eq!(h.app.profiles[0].sslmode, "verify");
    h.ctrl('p');
    h.key(KeyCode::Right);
    assert!(h.form().errors(|_| false).is_empty());
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(h.app.profiles[0].sslmode, "prefer");
}
