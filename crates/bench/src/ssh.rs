//! Round trips through an SSH tunnel: the latency proxy sits between the client
//! and the bastion of the tests, so it counts what crosses the SSH connection. The statements
//! must cost the round trips they cost directly: the tunnel adds latency per packet, never a
//! round trip. Opening the tunnel and the session through it are counted too.
//!
//! Needs the bastion (`dev/docker-compose.yml`, profile `ssh`) and its fixture:
//! `DATARIG_SSH_FIXTURE`, `DATARIG_TEST_SSH_BASTION` (`127.0.0.1:52222`), and the database
//! as the bastion sees it, `DATARIG_BENCH_SSH_TARGET` (default `postgres:5432`).

use crate::proxy;
use crate::rtt::{Cost, WiredSession, report, scenario};
use datarig_core::driver::{ConnectOptions, Driver, SessionRole};
use datarig_core::profile::ConnectionConfig;
use datarig_core::transport::{Dialer, DialerRef};
use datarig_driver_postgres::PgDriver;
use datarig_ssh::known_hosts::KnownHosts;
use datarig_ssh::tunnel::{HostKeyQuestion, Prompts, Secret, Stage};
use datarig_ssh::{Asker, Auth, Env, Hop, Options, Tunnel};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::unbounded_channel;

/// Trusts the bastion's key (in a scratch known_hosts) and asks nothing else.
struct Bench;

impl Asker for Bench {
    fn stage(&self, _: Stage) {}
    fn host_key(&self, _: HostKeyQuestion) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn passphrase(&self, _: PathBuf, _: bool) -> BoxFuture<'static, Option<Secret>> {
        Box::pin(async { None })
    }
    fn password(&self, _: bool) -> BoxFuture<'static, Option<Secret>> {
        Box::pin(async { None })
    }
    fn answers(&self, _: Prompts) -> BoxFuture<'static, Option<Vec<Secret>>> {
        Box::pin(async { None })
    }
}

fn var(k: &str) -> Result<String, String> {
    std::env::var(k).ok().filter(|v| !v.is_empty()).ok_or_else(|| format!("set {k} (see docs/perf.md)"))
}

fn host_port(s: &str) -> Result<(String, u16), String> {
    let (h, p) = s.rsplit_once(':').ok_or(format!("{s}: host:port"))?;
    Ok((h.to_string(), p.parse().map_err(|_| format!("{s}: host:port"))?))
}

pub async fn run(url: &str, scratch: &Path, one_way: Duration, runs: usize) -> Result<Value, String> {
    let fixture = PathBuf::from(var("DATARIG_SSH_FIXTURE")?);
    let (bastion, bastion_port) = host_port(&var("DATARIG_TEST_SSH_BASTION")?)?;
    let target = std::env::var("DATARIG_BENCH_SSH_TARGET").unwrap_or_else(|_| "postgres:5432".into());
    let (target_host, target_port) = host_port(&target)?;
    let d = datarig_core::profile::dsn::parse(url).map_err(|e| format!("{e:?}"))?;
    let p = proxy::start(bastion, bastion_port, one_way).await.map_err(|e| e.to_string())?;
    let hop = Hop {
        host: "127.0.0.1".into(),
        port: p.port,
        user: "tunnel".into(),
        auth: Auth::Key { path: fixture.join("id_ed25519"), passphrase: None },
    };
    let dir = scratch.join("ssh-bench");
    let _ = std::fs::remove_dir_all(&dir);
    let env = Env { known_hosts: KnownHosts { user: None, app: dir.join("known_hosts") }, agent: None };
    let options = Options { timeout: Duration::from_secs(10), keepalive: None };
    println!("rtt_ssh: {runs} runs per statement, {} ms one way, through the bastion to {target}", one_way.as_millis());
    let t0 = Instant::now();
    let tunnel = Tunnel::open(hop, options, env, Arc::new(Bench)).await.map_err(|e| format!("tunnel: {e:?}"))?;
    let open = Cost { to_result: p.counts.delivered(), total: p.counts.flights(), ms: crate::stats::ms(t0.elapsed()) };
    let dialer = DialerRef(Arc::new(tunnel) as Arc<dyn Dialer>);
    let cfg = ConnectionConfig {
        name: "bench-ssh".into(),
        host: target_host,
        port: target_port,
        user: d.user.clone(),
        password: d.password.clone().unwrap_or_default(),
        database: d.database.clone(),
        sslmode: "disable".into(),
        ..ConnectionConfig::default()
    };
    let before = p.counts.flights();
    let t1 = Instant::now();
    let (tx, rx) = unbounded_channel();
    let opts =
        ConnectOptions::new(500, SessionRole::Query, &format!("bench{}", std::process::id())).dialer(Some(dialer));
    let session = PgDriver.connect(&cfg, SessionRole::Query, opts, tx);
    let mut s = WiredSession::wired(session, rx, p.counts.clone(), one_way).await?;
    s.quiet().await;
    let connect = Cost {
        to_result: p.counts.flights() - before,
        total: p.counts.flights() - before,
        ms: crate::stats::ms(t1.elapsed()),
    };
    let mut out = vec![report("tunnel_open", &[open]), report("session_through_tunnel", &[connect])];
    s.cost("CREATE TEMP TABLE zz_bench_rtt (x int)").await?;
    let small = "SELECT id, event_type, created_at FROM analytics.events WHERE id <= 100";
    out.push(scenario(&mut s, "select_small_cold", runs, 0, |i| format!("{small} AND {i} >= 0")).await?);
    out.push(scenario(&mut s, "select_small_warm", runs, 1, |_| small.to_string()).await?);
    let big = "SELECT * FROM analytics.events";
    out.push(scenario(&mut s, "select_page_of_4m_warm", runs, 1, |_| big.to_string()).await?);
    let pages = s.next_pages(big, runs).await?;
    out.push(report("next_page", &pages));
    out.push(scenario(&mut s, "insert", runs, 0, |i| format!("INSERT INTO zz_bench_rtt VALUES ({i})")).await?);
    s.cost("DROP TABLE zz_bench_rtt").await?;
    Ok(json!({ "one_way_ms": one_way.as_millis() as u64, "scenarios": out }))
}
