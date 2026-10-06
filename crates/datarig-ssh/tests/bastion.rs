//! A real OpenSSH bastion (`dev/docker-compose.yml`, service `ssh-bastion`, profile
//! `ssh`) in front of the test PostgreSQL, which the host cannot reach by the name the tunnel
//! uses (`postgres`): sessions, pages and cancels of the PostgreSQL driver through the tunnel,
//! with an OpenSSH key, an AWS-style PEM key and a password.
//!
//! Needs `DATARIG_SSH_FIXTURE` (the directory `dev/ssh/make-fixture.sh` wrote, which
//! the container mounts) and `DATARIG_TEST_SSH_BASTION` (`127.0.0.1:52222`). Unset locally:
//! each test prints a visible `SKIPPED` line; with `DATARIG_REQUIRE_SSH=1` (CI) it fails.

mod common;

use common::{Answers, TestAsker, scratch};
use datarig_core::driver::PagingMode;
use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Driver, SessionRole};
use datarig_core::profile::ConnectionConfig;
use datarig_core::transport::{DialError, Dialer, DialerRef, Refusal};
use datarig_driver_postgres::PgDriver;
use datarig_ssh::known_hosts::{KnownHosts, fingerprint};
use datarig_ssh::tunnel::Secret;
use datarig_ssh::{Auth, Env, Hop, Options, Tunnel};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::unbounded_channel;

struct Bastion {
    fixture: PathBuf,
    host: String,
    port: u16,
}

fn bastion(test: &str) -> Option<Bastion> {
    let fixture = std::env::var("DATARIG_SSH_FIXTURE").ok().filter(|v| !v.is_empty());
    let addr = std::env::var("DATARIG_TEST_SSH_BASTION").ok().filter(|v| !v.is_empty());
    match (fixture, addr) {
        (Some(f), Some(a)) => {
            let (host, port) = a.rsplit_once(':').expect("host:port");
            Some(Bastion { fixture: PathBuf::from(f), host: host.into(), port: port.parse().unwrap() })
        }
        _ => {
            if std::env::var("DATARIG_REQUIRE_SSH").is_ok_and(|v| v == "1") {
                panic!("DATARIG_SSH_FIXTURE and DATARIG_TEST_SSH_BASTION must be set when DATARIG_REQUIRE_SSH=1");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: run dev/ssh/make-fixture.sh, `docker compose --profile ssh up -d --build \
                 ssh-bastion`, then set DATARIG_SSH_FIXTURE=<fixture dir> DATARIG_TEST_SSH_BASTION=127.0.0.1:52222"
            );
            None
        }
    }
}

impl Bastion {
    fn hop(&self, user: &str, auth: Auth) -> Hop {
        Hop { host: self.host.clone(), port: self.port, user: user.into(), auth }
    }

    fn key(&self, name: &str) -> Auth {
        Auth::Key { path: self.fixture.join(name), passphrase: None }
    }

    /// The bastion's host key fingerprints (from the fixture).
    fn fingerprints(&self) -> Vec<String> {
        ["ssh_host_ed25519_key.pub", "ssh_host_rsa_key.pub"]
            .iter()
            .map(|n| {
                let text = std::fs::read_to_string(self.fixture.join(n)).unwrap();
                fingerprint(&russh::keys::PublicKey::from_openssh(text.trim()).unwrap())
            })
            .collect()
    }
}

fn env(dir: &std::path::Path) -> Env {
    Env { known_hosts: KnownHosts { user: None, app: dir.join("known_hosts") }, agent: None }
}

fn trusting() -> Arc<TestAsker> {
    TestAsker::new(Answers { trust: true, ..Answers::default() })
}

/// The test database as the bastion sees it.
fn db_behind() -> ConnectionConfig {
    ConnectionConfig { name: "behind".into(), host: "postgres".into(), port: 5432, ..ConnectionConfig::test_db() }
}

async fn open(b: &Bastion, user: &str, auth: Auth, dir: &std::path::Path) -> Arc<Tunnel> {
    Arc::new(Tunnel::open(b.hop(user, auth), Options::default(), env(dir), trusting()).await.expect("tunnel"))
}

#[tokio::test(flavor = "multi_thread")]
async fn every_login_reaches_the_database_behind_the_bastion() {
    let Some(b) = bastion("every_login_reaches_the_database_behind_the_bastion") else { return };
    let d = scratch("bastion-logins");
    // The bastion (OpenSSH 10) takes no `ssh-rsa` (SHA-1) signatures: the PEM key logs in with
    // `rsa-sha2-*`.
    for (user, auth) in [
        ("tunnel", b.key("id_ed25519")),
        ("tunnel", b.key("id_rsa.pem")),
        ("pw", Auth::Password(Some(Secret::new("datarig-pw".into())))),
    ] {
        let asker = trusting();
        let tunnel = Tunnel::open(b.hop(user, auth.clone()), Options::default(), env(&d), asker.clone())
            .await
            .unwrap_or_else(|e| panic!("{user} {auth:?}: {e:?}"));
        let asked = asker.seen.lock().unwrap().host_keys.clone();
        // Asked once (the first login), with the bastion's real key.
        for q in &asked {
            assert!(b.fingerprints().contains(&q.fingerprint), "{q:?}");
        }
        let dialer = Some(DialerRef(Arc::new(tunnel) as Arc<dyn Dialer>));
        let info = PgDriver.ping(&db_behind(), Duration::from_secs(10), dialer).await.expect("ping through the tunnel");
        assert!(info.server_version.starts_with(|c: char| c.is_ascii_digit()));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_pages_and_cancels_through_the_bastion() {
    let Some(b) = bastion("a_session_pages_and_cancels_through_the_bastion") else { return };
    let d = scratch("bastion-session");
    let tunnel = open(&b, "tunnel", b.key("id_rsa.pem"), &d).await;
    let dialer = Some(DialerRef(tunnel.clone() as Arc<dyn Dialer>));
    let (tx, mut rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, SessionRole::Query, "bastion").dialer(dialer);
    let session = PgDriver.connect(&db_behind(), SessionRole::Query, opts, tx);
    let mut next =
        async || tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("event").expect("open");
    assert!(matches!(next().await, DbEvent::Connected));
    session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT g FROM generate_series(1, 250) g".into()],
        paging: PagingMode::Hold,
    });
    let mut rows = 0;
    loop {
        match next().await {
            DbEvent::Page { rows: r, more, .. } => {
                rows += r.len();
                if !more {
                    break;
                }
                session.send(DbCommand::FetchMore { id: 1 });
            }
            DbEvent::Failed { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    }
    assert_eq!(rows, 250);
    session.send(DbCommand::Execute {
        id: 2,
        statements: vec!["SELECT pg_sleep(60)".into()],
        paging: PagingMode::Hold,
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let t0 = Instant::now();
    session.cancel();
    loop {
        match next().await {
            DbEvent::Failed { id: 2, cancelled, .. } => {
                assert!(cancelled, "cancelled through the tunnel");
                break;
            }
            DbEvent::Page { id: 2, .. } | DbEvent::Done { id: 2, .. } => panic!("not cancelled"),
            _ => {}
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(10));
    session.close();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bastion_forwards_only_where_it_allows() {
    let Some(b) = bastion("the_bastion_forwards_only_where_it_allows") else { return };
    let d = scratch("bastion-permit");
    let tunnel = open(&b, "tunnel", b.key("id_ed25519"), &d).await;
    // `PermitOpen` lists postgres and pgbouncer only.
    match tunnel.dial("mysql", 3306).await {
        Err(DialError::Refused { reason: Refusal::Prohibited, .. }) => {}
        other => panic!("{:?}", other.err()),
    }
    // Through the pooler behind the bastion too.
    let pooler = ConnectionConfig { host: "pgbouncer".into(), ..db_behind() };
    let dialer = Some(DialerRef(tunnel.clone() as Arc<dyn Dialer>));
    PgDriver.ping(&pooler, Duration::from_secs(10), dialer).await.expect("pgbouncer through the tunnel");
}
