//! Keeping the test database clean (shared by the `integration_pg` targets of the driver and
//! TUI crates; the TUI one includes this file with `#[path]`).
//!
//! * Tables a test creates are named `it_<what>_<tag>`, where the tag ends with the process id,
//!   and live in `public`. Tests that do not need another session to see the table use a
//!   `TEMP` table instead (it goes away with its connection).
//! * [`TableGuard`] drops the tables when the test ends, on a fresh connection, also when an
//!   assertion panics.
//! * [`sweep_stale`] runs once per test process before the first test uses the database: it
//!   drops `public.it_%` tables left by earlier runs that were killed before their guard ran.
//!   A table whose process id still names a connected `application_name` belongs to a test
//!   that runs right now and is kept. So that this holds at every moment of a live process
//!   (test runs may go in parallel), each process then holds one connection for its whole
//!   life, tagged with its process id ([`keep_alive`]), before any of its tests creates a
//!   table.
//! * [`hold_slot`]: at most a few tests of one process use the database at a time, so several
//!   test runs in parallel stay under the server's `max_connections` (100 by default; an app
//!   test opens several connections).
//! * CI checks after `cargo test` that `public` has no tables left; locally
//!   `psql … -c "\dt public.*"` should list none.

use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Driver, SessionRole};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_postgres::PgDriver;
use std::cell::Cell;
use std::io::Write;
use std::sync::{Condvar, Mutex, Once};
use std::time::Duration;

/// Tests of this process that use the database now, and the signal that one ended.
static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

thread_local! {
    /// This thread (one test) holds a slot; given back when the thread ends.
    static HELD: SlotGuard = const { SlotGuard(Cell::new(false)) };
}

struct SlotGuard(Cell<bool>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if self.0.get() {
            let (count, freed) = &SLOTS;
            *count.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            freed.notify_one();
        }
    }
}

/// Wait until fewer than `DATARIG_TEST_PG_SLOTS` (default 2) tests of this process use the
/// database, then count this test (its thread) in until it ends. A thread that already holds
/// a slot (tests run on one thread with `--test-threads=1`) keeps it.
pub fn hold_slot() {
    let max = std::env::var("DATARIG_TEST_PG_SLOTS").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(2);
    HELD.with(|held| {
        if held.0.get() {
            return;
        }
        let (count, freed) = &SLOTS;
        let mut n = count.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= max {
            n = freed.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        held.0.set(true);
    });
}

/// Run `sql` on a fresh connection, on a thread and runtime of its own: callable from `Drop`
/// while a test unwinds, inside or outside a Tokio runtime.
pub fn run_fresh(url: &str, sql: &str) -> Result<(), String> {
    let (url, sql) = (url.to_string(), sql.to_string());
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let cfg = ConnectionConfig { name: "it-clean".into(), dsn: Some(url), ..ConnectionConfig::test_db() };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let opts = ConnectOptions::new(1, SessionRole::Query, "it-clean");
            let session = PgDriver.connect(&cfg, SessionRole::Query, opts, tx);
            session.send(DbCommand::Execute { id: 1, statements: vec![sql] });
            loop {
                match tokio::time::timeout(Duration::from_secs(20), rx.recv()).await {
                    Ok(Some(DbEvent::Done { .. } | DbEvent::Page { .. })) => return Ok(()),
                    Ok(Some(DbEvent::Failed { error, .. } | DbEvent::ConnectFailed { error, .. })) => {
                        return Err(format!("{error:?}"));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => return Err("the cleanup connection ended".into()),
                    Err(_) => return Err("the cleanup statement did not finish within 20s".into()),
                }
            }
        })
    });
    worker.join().map_err(|_| "the cleanup thread panicked".to_string())?
}

/// Drops its tables (`DROP TABLE IF EXISTS`) on a fresh connection when it goes out of scope.
/// Create it before the tables, so it is dropped after the sessions that use them.
pub struct TableGuard {
    url: String,
    tables: Vec<String>,
}

impl TableGuard {
    pub fn new(url: &str, tables: &[&str]) -> Self {
        Self { url: url.to_string(), tables: tables.iter().map(|t| t.to_string()).collect() }
    }
}

impl Drop for TableGuard {
    fn drop(&mut self) {
        let sql = format!("DROP TABLE IF EXISTS {}", self.tables.join(", "));
        if let Err(e) = run_fresh(&self.url, &sql) {
            // Never panic here: this may run while a failed assertion unwinds.
            let _ = writeln!(std::io::stderr(), "warning: {sql}: {e}");
        }
    }
}

/// Drop `public.it_%` tables of test runs that are gone (see the module docs). Once per process.
pub fn sweep_stale(url: &str) {
    static SWEEP: Once = Once::new();
    SWEEP.call_once(|| {
        let sql = r"DO $$
DECLARE r record;
BEGIN
  FOR r IN
    SELECT c.relname FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND c.relname LIKE 'it\_%'
      AND NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_stat_activity a
        WHERE a.application_name LIKE '%' || substring(c.relname FROM '([0-9]+)$'))
  LOOP
    EXECUTE format('DROP TABLE IF EXISTS public.%I', r.relname);
  END LOOP;
END $$";
        if let Err(e) = run_fresh(url, sql) {
            let _ = writeln!(std::io::stderr(), "warning: sweeping stale it_ tables: {e}");
        }
        keep_alive(url);
    });
}

/// Open a connection tagged with this process's id and hold it until the process ends (on a
/// thread and runtime of its own), so another process's [`sweep_stale`] never takes this
/// process's tables for a dead run's. Returns once it is connected.
fn keep_alive(url: &str) {
    let url = url.to_string();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(async move {
            let cfg = ConnectionConfig { name: "it-keep".into(), dsn: Some(url), ..ConnectionConfig::test_db() };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let opts = ConnectOptions::new(1, SessionRole::Query, &format!("it-keep{}", std::process::id()));
            let _session = PgDriver.connect(&cfg, SessionRole::Query, opts, tx);
            let connected = matches!(rx.recv().await, Some(DbEvent::Connected));
            let _ = ready_tx.send(connected);
            std::future::pending::<()>().await;
        });
    });
    if !matches!(ready_rx.recv_timeout(Duration::from_secs(20)), Ok(true)) {
        let _ = writeln!(std::io::stderr(), "warning: no keep-alive connection; a parallel run may sweep our tables");
    }
}
