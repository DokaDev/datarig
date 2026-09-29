//! A profile through the real SSH bastion of the tests, driven through `App`
//! like the binary does: the explorer connects, the host key is asked about and trusted in
//! datarig's own file, the database behind the bastion answers, a lost tunnel shows on the
//! node and the next use connects again.
//!
//! Needs the bastion of `dev/docker-compose.yml` (profile `ssh`) and its fixture:
//! `DATARIG_SSH_FIXTURE` and `DATARIG_TEST_SSH_BASTION` (see `datarig-ssh/tests/bastion.rs`).
//! Unset locally: a visible `SKIPPED` line; with `DATARIG_REQUIRE_SSH=1` (CI) the test fails.
//! Home, data and state directories are scratch ones: the user's `~/.ssh` is never read.

use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::overlay::ConfirmAction;
use datarig_tui::app::tunnel::SshTunnels;
use datarig_tui::app::{App, AppEvent, Startup};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

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
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: no SSH bastion (see datarig-ssh/tests/bastion.rs)");
            None
        }
    }
}

/// A scratch directory of the test, removed with everything in it when it goes out of scope
/// (also when the test fails).
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-tui-ssh-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

async fn pump(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, secs: u64, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done(app) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) => app.on_app_event(ev),
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out after {secs}s (status {:?}, notices {:?})", app.status, app.notices),
        }
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_connects_through_the_bastion() {
    let Some(b) = bastion("a_profile_connects_through_the_bastion") else { return };
    let home = scratch("home");
    let data = scratch("data");
    let profile = ConnectionConfig {
        name: "behind-bastion".into(),
        host: "postgres".into(),
        port: 5432,
        ssh: Some(SshSettings {
            enabled: true,
            host: b.host.clone(),
            port: b.port,
            user: "tunnel".into(),
            auth: SshAuth::Key,
            key_file: Some(b.fixture.join("id_rsa.pem").display().to_string()),
            ..SshSettings::default()
        }),
        ..ConnectionConfig::test_db()
    };
    let store = Arc::new(MemoryStore::new());
    store.set(&profile.id.account(), "datarig").unwrap();
    let id = profile.id;
    let cfg = Config { connections: vec![profile], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    let state = scratch("state");
    app.set_paths(Paths { data: Some(data.to_path_buf()), state: Some(state.to_path_buf()) });
    let h = home.to_path_buf();
    app.set_env_lookup(Arc::new(move |k: &str| (k == "HOME").then(|| h.display().to_string())));
    app.set_tunnels(Arc::new(SshTunnels));
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    key(&mut app, KeyCode::Enter);
    // The bastion is new: asked about, and trusted.
    pump(&mut app, &mut rx, 20, |a| a.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::TrustHostKey))
        .await;
    key(&mut app, KeyCode::Char('y'));
    pump(&mut app, &mut rx, 20, |a| a.conns.is_connected(id)).await;
    let known = std::fs::read_to_string(data.join("known_hosts")).expect("trusted in datarig's file");
    assert!(known.contains(&format!("[{}]:{}", b.host, b.port)), "{known}");
    assert!(!home.join(".ssh").exists(), "the user's ~/.ssh is never written");
    pump(&mut app, &mut rx, 20, |a| a.conns.catalog(Some(id)).schemas.iter().any(|s| s == "shop")).await;
    // Connecting again later (after `x`) asks nothing: the key is known now.
    key(&mut app, KeyCode::Char('x'));
    assert!(!app.conns.is_connected(id));
    key(&mut app, KeyCode::Enter);
    pump(&mut app, &mut rx, 20, |a| a.conns.is_connected(id) || a.overlays.confirm().is_some()).await;
    assert!(app.overlays.confirm().is_none(), "not asked again");
}

/// A TCP proxy to the bastion that can cut every connection through it (the test's own
/// connections only; the bastion and other clients are left alone).
struct Cutter {
    port: u16,
    conns: Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Cutter {
    async fn start(to: (String, u16)) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conns: Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>> = Default::default();
        let c = conns.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                let to = to.clone();
                let h = tokio::spawn(async move {
                    if let Ok(mut out) = tokio::net::TcpStream::connect((to.0.as_str(), to.1)).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut out).await;
                    }
                });
                c.lock().unwrap().push(h.abort_handle());
            }
        });
        Cutter { port, conns, task }
    }

    /// End every connection through it (the proxy keeps listening).
    fn cut(&self) {
        for h in self.conns.lock().unwrap().drain(..) {
            h.abort();
        }
    }
}

impl Drop for Cutter {
    fn drop(&mut self) {
        self.cut();
        self.task.abort();
    }
}

/// The tunnel is lost while the user's transaction is open: the next statement connects the
/// profile again (a new tunnel), runs once, and says the transaction is gone.
#[tokio::test(flavor = "multi_thread")]
async fn the_next_statement_after_a_lost_tunnel_connects_again() {
    let Some(b) = bastion("the_next_statement_after_a_lost_tunnel_connects_again") else { return };
    let cutter = Cutter::start((b.host.clone(), b.port)).await;
    let home = scratch("lost-home");
    let data = scratch("lost-data");
    let profile = ConnectionConfig {
        name: "behind-bastion".into(),
        host: "postgres".into(),
        port: 5432,
        ssh: Some(SshSettings {
            enabled: true,
            host: "127.0.0.1".into(),
            port: cutter.port,
            user: "tunnel".into(),
            auth: SshAuth::Key,
            key_file: Some(b.fixture.join("id_rsa.pem").display().to_string()),
            keepalive: Some(1),
            ..SshSettings::default()
        }),
        ..ConnectionConfig::test_db()
    };
    let store = Arc::new(MemoryStore::new());
    store.set(&profile.id.account(), "datarig").unwrap();
    let id = profile.id;
    let cfg = Config { connections: vec![profile], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    let state = scratch("lost-state");
    app.set_paths(Paths { data: Some(data.to_path_buf()), state: Some(state.to_path_buf()) });
    let h = home.to_path_buf();
    app.set_env_lookup(Arc::new(move |k: &str| (k == "HOME").then(|| h.display().to_string())));
    app.set_tunnels(Arc::new(SshTunnels));
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    key(&mut app, KeyCode::Enter);
    pump(&mut app, &mut rx, 20, |a| a.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::TrustHostKey))
        .await;
    key(&mut app, KeyCode::Char('y'));
    pump(&mut app, &mut rx, 20, |a| a.conns.is_connected(id) && !a.tabs.is_empty()).await;
    let run = |app: &mut App, sql: &str| {
        app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(sql);
        app.execute_current();
    };
    run(&mut app, "BEGIN");
    pump(&mut app, &mut rx, 20, |a| a.tab().exec.in_block && a.tab().exec.running.is_none()).await;
    run(&mut app, "SELECT 'before'");
    pump(&mut app, &mut rx, 20, |a| {
        a.tab().exec.running.is_none() && matches!(a.tab().results, datarig_tui::app::Results::Rows(_))
    })
    .await;
    // The tunnel's TCP connection ends. The sessions through it may see their end before the
    // tunnel's own loss arrives (then they say the server closed); the tunnel's words follow.
    cutter.cut();
    let error = |a: &App| a.conns.get(id).and_then(|c| c.error.as_ref()).map(|e| e.render(&a.i18n).to_string());
    pump(&mut app, &mut rx, 30, |a| {
        !a.conns.is_connected(id) && a.tab().exec.session.is_none() && error(a).is_some_and(|e| e.starts_with("SSH:"))
    })
    .await;
    let lost = error(&app);
    assert!(lost.as_deref().is_some_and(|e| e.starts_with("SSH:")), "{lost:?}");
    // The next statement connects again and runs once.
    run(&mut app, "SELECT 40 + 2");
    pump(&mut app, &mut rx, 30, |a| {
        a.tab().exec.running.is_none()
            && match &a.tab().results {
                datarig_tui::app::Results::Rows(rs) => matches!(
                    rs.cell(0, 0),
                    datarig_tui::widgets::grid::CellRef::Here(Some(v)) if v == "42"
                ),
                _ => false,
            }
    })
    .await;
    assert!(app.conns.is_connected(id));
    assert!(!app.tab().exec.in_block, "the user's transaction did not come back");
    let notices: Vec<String> = app.notices.iter().map(|n| n.render(&app.i18n).to_string()).collect();
    let status = app.status.as_ref().map(|s| s.render(&app.i18n).to_string()).unwrap_or_default();
    let said =
        notices.iter().chain(std::iter::once(&status)).any(|n| n.contains("the transaction that was open is gone"))
            || app
                .transient
                .as_ref()
                .is_some_and(|(f, _)| f.render(&app.i18n).contains("the transaction that was open is gone"));
    assert!(said, "said: {notices:?} {status:?}");
    let run_log: Vec<&str> = app.tab().exec.run.statements.iter().map(|s| s.sql.as_str()).collect();
    assert_eq!(run_log, ["SELECT 40 + 2"], "only the new statement ran");
}
