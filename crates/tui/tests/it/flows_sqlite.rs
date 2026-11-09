//! A SQLite profile through the real app and the real SQLite driver: connect, run a query, export
//! every row to a file. The export is written to `<file>.partial` and renamed when it ends, so no
//! `.partial` file is left behind. Real database files in a scratch folder.
use crate::common;
use common::*;
use datarig_core::config::Config;
use datarig_core::profile::ConnectionConfig;
use datarig_i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{AppEvent, Startup};
use ratatui::crossterm::event::KeyCode;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(20);

/// A new empty folder for a test's files.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-sqlite-flow-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A database file with a table `n` of `rows` rows.
fn database(dir: &Path, rows: u64) -> PathBuf {
    let file = dir.join("shop.db");
    let conn = rusqlite::Connection::open(&file).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE n (x INTEGER); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < {rows}) INSERT INTO n SELECT x FROM c;"
    ))
    .unwrap();
    file
}

fn sqlite_config(file: &Path) -> Config {
    let profile = ConnectionConfig {
        name: "shop".into(),
        driver: "sqlite".into(),
        path: Some(file.display().to_string()),
        ..ConnectionConfig::default()
    };
    Config { connections: vec![profile], ..Config::default() }
}

/// Feeds the app the events the sessions sent, until `done` holds.
fn pump(h: &mut Harness, rx: &mut UnboundedReceiver<AppEvent>, what: &str, done: impl Fn(&Harness) -> bool) {
    let start = Instant::now();
    loop {
        while let Ok(event) = rx.try_recv() {
            h.app.on_app_event(event);
        }
        h.app.pump_exports(Duration::from_millis(10));
        if done(h) {
            return;
        }
        assert!(
            start.elapsed() < WAIT,
            "waited for {what}: {:?}",
            h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string())
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_export_of_a_sqlite_result_is_renamed_from_its_partial_file() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let _inside = runtime.enter();
    let dir = scratch("export");
    let file = database(&dir, 2000);
    let (mut h, mut rx) = Harness::started(
        &sqlite_config(&file),
        Lang::En,
        Arc::new(datarig_core::secret::MemoryStore::new()),
        Startup::Normal,
    );

    h.explore("shop");
    h.key(KeyCode::Enter);
    pump(&mut h, &mut rx, "the connection", |h| h.app.tab().profile.is_some());
    h.app.focus = datarig_tui::app::Focus::Editor;
    h.keys("i");
    h.type_text("SELECT x FROM n ORDER BY x");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    pump(&mut h, &mut rx, "the result", |h| matches!(h.app.tab().results, datarig_tui::app::Results::Rows(_)));

    let out = dir.join("all.csv");
    h.command(&format!("export all {}", out.display()));
    pump(&mut h, &mut rx, "the export", |h| !h.app.exporting(h.app.tab().id));

    assert!(!dir.join("all.csv.partial").exists(), "the .partial file was left behind");
    let text = std::fs::read_to_string(&out).expect("the exported file");
    assert_eq!(text.lines().count(), 2001, "a header and 2000 rows");
    assert!(text.lines().nth(1).unwrap().starts_with('1'));
}

/// Switching a PostgreSQL profile's driver to SQLite redraws the form: the file is asked for and
/// no server field (host, port, login, DSN) is on screen.
#[test]
fn switching_the_driver_to_sqlite_draws_the_file_and_no_server_field() {
    let pg = ConnectionConfig { name: "pg".into(), driver: "postgres".into(), ..ConnectionConfig::default() };
    let cfg = Config { connections: vec![pg], ..Config::default() };
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(datarig_core::secret::MemoryStore::new()), Startup::Normal);
    h.explore("pg");
    h.keys("e");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm));
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Enter);
    h.type_text("sqlite");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().driver, 2, "SQLite is picked");
    let screen = h.screen(80, 24);
    assert!(screen.contains("File"), "the file is asked for:\n{screen}");
    for server in ["Host", "Port", "DSN"] {
        assert!(!screen.contains(server), "{server} is still drawn:\n{screen}");
    }
}
