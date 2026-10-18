//! Exports end to end against a real PostgreSQL (`DATARIG_TEST_PG_URL`): the app, the tab's
//! session and the writer, into files in a scratch folder. Every row of a result past its first
//! page streams from the session; a table tab is read in chunks by its key; a read-only
//! environment asks first and then exports. Tables are `public.zz_export_app_*` of this process.

#[allow(dead_code)]
#[path = "../../drivers/postgres/tests/pg_clean/mod.rs"]
mod pg_clean;

use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::tabs::TableRef;
use datarig_tui::app::{App, AppEvent, Startup};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn pg_url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => {
            pg_clean::sweep_stale(&u);
            pg_clean::hold_slot();
            Some(u)
        }
        _ => {
            if std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_PG_URL must be set when DATARIG_REQUIRE_PG=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set DATARIG_TEST_PG_URL");
            None
        }
    }
}

/// Drops a table on a fresh connection when the test ends.
struct TableGuard(String, String);

impl Drop for TableGuard {
    fn drop(&mut self) {
        let _ = pg_clean::run_fresh(&self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-export-pg-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn app(url: &str, tag: &str, policy: Option<&str>) -> (App, UnboundedReceiver<AppEvent>) {
    let d = datarig_core::profile::dsn::parse(url).expect("a postgres:// URL");
    let p = ConnectionConfig {
        name: "it-pg".into(),
        host: d.host.clone(),
        port: d.port.unwrap_or(5432),
        user: d.user.clone(),
        password: String::new(),
        database: d.database.clone(),
        sslmode: "disable".into(),
        policy: policy.map(str::to_string),
        ..ConnectionConfig::test_db()
    };
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &d.password.unwrap_or_default()).unwrap();
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("it-ro", Policy { read_only: true, ..Policy::default() });
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_instance_tag(tag);
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let id = app.profiles[0].id;
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    pump(&mut app, &mut rx, 15, |a| a.conns.is_connected(id)).await;
    (app, rx)
}

async fn pump(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, secs: u64, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done(app) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) => app.on_app_event(ev),
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out after {secs}s (status {:?})", app.tab().status),
        }
    }
}

fn status(app: &App) -> String {
    app.tab().status.as_ref().map(|n| n.render(&app.i18n).to_string()).unwrap_or_default()
}

async fn run(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, sql: &str) {
    app.run(vec![sql.to_string()]);
    pump(app, rx, 20, |a| a.tab().exec.running.is_none()).await;
}

async fn export(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, arg: &str) {
    let req = app.export_request(arg).unwrap();
    app.start_export(req);
    let id = app.tab().id;
    pump(app, rx, 60, |a| !a.exporting(id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_row_of_a_result_goes_to_the_file() {
    let Some(url) = pg_url("every_row_of_a_result_goes_to_the_file") else { return };
    let t = format!("public.zz_export_app_all_{}", std::process::id());
    let _g = TableGuard(url.clone(), t.clone());
    pg_clean::run_fresh(
        &url,
        &format!("CREATE TABLE {t} AS SELECT g AS id, chr(54620) || g AS v FROM generate_series(1, 30000) g"),
    )
    .unwrap();
    let dir = scratch("all");
    let (mut app, mut rx) = app(&url, &format!("exa{}", std::process::id()), None).await;
    run(&mut app, &mut rx, &format!("SELECT * FROM {t} ORDER BY id")).await;
    let path = dir.join("all.csv");
    export(&mut app, &mut rx, &format!("all {}", path.display())).await;
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 30_001, "{}", status(&app));
    assert_eq!(lines[0], "id,v");
    assert_eq!(lines[30_000], "30000,\u{d55c}30000");
    assert!(status(&app).starts_with("Exported 30,000 rows to "), "{}", status(&app));
    assert!(app.tab().exec.running.is_none());
    // The tab's session goes on.
    run(&mut app, &mut rx, "SELECT 7").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_table_tab_exports_by_its_key() {
    let Some(url) = pg_url("a_table_tab_exports_by_its_key") else { return };
    let name = format!("zz_export_app_key_{}", std::process::id());
    let t = format!("public.{name}");
    let _g = TableGuard(url.clone(), t.clone());
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {t} (id int PRIMARY KEY, v text)")).unwrap();
    pg_clean::run_fresh(&url, &format!("INSERT INTO {t} SELECT g, 'r' || g FROM generate_series(1, 25000) g")).unwrap();
    let dir = scratch("key");
    let (mut app, mut rx) = app(&url, &format!("exk{}", std::process::id()), None).await;
    let table = TableRef { schema: "public".into(), name: name.clone() };
    let sql = table.query(datarig_core::sql::dialect::Dialect::Postgres);
    app.tab_mut().doc.table = Some(table);
    run(&mut app, &mut rx, &sql).await;
    let path = dir.join("key.jsonl");
    export(&mut app, &mut rx, &format!("{}", path.display())).await;
    let text = std::fs::read_to_string(&path).unwrap();
    let ids: Vec<i64> =
        text.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["id"].as_i64().unwrap()).collect();
    assert_eq!(ids.len(), 25_000, "{}", status(&app));
    assert!(ids.iter().enumerate().all(|(i, id)| *id == i as i64 + 1), "every row once, in key order");
    let notes: Vec<String> = app.tab().exec.run.notes.iter().map(|n| n.render(&app.i18n).to_string()).collect();
    assert!(notes.iter().any(|n| n.starts_with("Exported 25,000 rows")), "{notes:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_environment_asks_then_exports() {
    let Some(url) = pg_url("a_read_only_environment_asks_then_exports") else { return };
    let dir = scratch("ro");
    let (mut app, mut rx) = app(&url, &format!("exr{}", std::process::id()), Some("it-ro")).await;
    run(&mut app, &mut rx, "SELECT g FROM generate_series(1, 2000) g").await;
    let path = dir.join("ro.md");
    let req = app.export_request(&format!("all {}", path.display())).unwrap();
    app.start_export(req);
    assert!(app.overlays.confirm().is_some(), "a read-only environment asks first");
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)));
    let id = app.tab().id;
    pump(&mut app, &mut rx, 60, |a| !a.exporting(id)).await;
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2_002, "{}", status(&app));
}
