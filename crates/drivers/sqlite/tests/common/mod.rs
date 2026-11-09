//! Helpers shared by the SQLite driver's tests: a scratch directory, a profile, a session and
//! its runs. Each test file includes this module.
#![allow(dead_code)]

use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, Session, SessionRole, TxRule,
};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_sqlite::SqliteDriver;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// A directory for one test's files, removed when the test ends.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "datarig-sqlite-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A profile naming `path`.
pub fn profile(path: &str) -> ConnectionConfig {
    ConnectionConfig { driver: "sqlite".into(), path: Some(path.to_string()), ..ConnectionConfig::default() }
}

/// Creates a database file with `setup` run on it (before the driver opens it).
pub fn create(file: &Path, setup: &str) {
    let conn = rusqlite::Connection::open(file).unwrap();
    conn.execute_batch(setup).unwrap();
}

/// Opens a session on `file` and waits for it to connect.
pub fn connect(file: &Path, role: SessionRole, read_only: bool) -> (Session, UnboundedReceiver<DbEvent>) {
    let (tx, mut rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, role, "test").read_only(read_only);
    let session = SqliteDriver.connect(&profile(&file.display().to_string()), role, opts, tx);
    match rx.blocking_recv() {
        Some(DbEvent::Connected) => {}
        other => panic!("expected Connected, got {other:?}"),
    }
    // A metadata session sends its schema list and its catalog as soon as it is up.
    if role == SessionRole::Meta {
        assert!(matches!(rx.blocking_recv(), Some(DbEvent::Schemas(Ok(_)))), "the schema list first");
        assert!(matches!(rx.blocking_recv(), Some(DbEvent::Catalog(Ok(_)))), "then the catalog");
    }
    (session, rx)
}

/// Runs one statement as a run and returns its events up to the run's answer.
pub fn run(session: &Session, rx: &mut UnboundedReceiver<DbEvent>, id: u64, sql: &str) -> Vec<DbEvent> {
    session.send(DbCommand::Execute {
        id,
        statements: vec![sql.to_string()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    until_answer(rx, id)
}

pub fn until_answer(rx: &mut UnboundedReceiver<DbEvent>, id: u64) -> Vec<DbEvent> {
    let mut events = Vec::new();
    loop {
        let event = rx.blocking_recv().expect("the session ended before answering");
        let answer = matches!(
            event,
            DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } | DbEvent::Page { id: i, more: false, .. }
            if i == id
        );
        events.push(event);
        if answer {
            return events;
        }
    }
}

/// The failure message of a run that failed.
pub fn failure(events: &[DbEvent]) -> (String, bool) {
    match events.last() {
        Some(DbEvent::Failed { error: DbError::Server(msg), cancelled, .. }) => (msg.clone(), *cancelled),
        Some(DbEvent::Failed { error, cancelled, .. }) => (format!("{error:?}"), *cancelled),
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// The rows of a run's answer (its page).
pub fn page_rows(events: &[DbEvent]) -> Vec<Vec<Option<String>>> {
    match events.last() {
        Some(DbEvent::Page { rows, .. }) => rows.clone(),
        other => panic!("expected a page, got {other:?}"),
    }
}
