//! The driver: what it can do, the sessions it opens, and the test connection.

use crate::open;
use crate::session::{self, SqliteCanceller};
use datarig_core::driver::{
    Capabilities, ConnectOptions, DbError, DbEvent, Driver, Hierarchy, PingError, PingInfo, Session, SessionRole,
};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::tls::Encryption;
use datarig_core::transport::DialerRef;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub struct SqliteDriver;

impl Driver for SqliteDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // A result is read in one piece (the rows of a larger export are streamed by the
            // export itself): there is no server-side cursor to page through.
            server_paging: false,
            cancel: true,
            introspection: true,
            key_metadata: false,
            contexts: false,
            structure: false,
            ddl: true,
            language: Language::Sql(Dialect::Sqlite),
            hierarchy: Hierarchy::SchemaOnly,
            explain: None,
            reads_held_to_end: false,
            export: true,
            manual_commit: false,
            edit: None,
            monitor: false,
        }
    }

    fn connect(
        &self,
        cfg: &ConnectionConfig,
        role: SessionRole,
        opts: ConnectOptions,
        events: UnboundedSender<DbEvent>,
    ) -> Session {
        let (tx, rx) = unbounded_channel();
        let canceller = Arc::new(SqliteCanceller::default());
        match open::file_of(cfg) {
            Err(error) => {
                let _ = events.send(DbEvent::ConnectFailed { error, auth: false });
            }
            Ok(file) => {
                let weak = Arc::downgrade(&canceller);
                let failing = events.clone();
                let spawned = std::thread::Builder::new()
                    .name("datarig-sqlite".to_string())
                    .spawn(move || session::run(file, role, opts, events, rx, weak));
                if let Err(e) = spawned {
                    let error = DbError::Server(format!("cannot start the session's thread: {e}"));
                    let _ = failing.send(DbEvent::ConnectFailed { error, auth: false });
                }
            }
        }
        Session::new(self.capabilities(), role, tx, canceller)
    }

    fn ping(
        &self,
        cfg: &ConnectionConfig,
        timeout: Duration,
        dialer: Option<DialerRef>,
    ) -> BoxFuture<'static, Result<PingInfo, PingError>> {
        let file = match open::file_of(cfg) {
            Ok(file) => file,
            Err(error) => return Box::pin(async move { Err(PingError::Failed(error)) }),
        };
        // A file has no route for a tunnel to carry: a dialer cannot apply.
        if dialer.is_some() {
            return Box::pin(async { Err(PingError::Failed(DbError::NotSupported)) });
        }
        Box::pin(async move {
            let start = Instant::now();
            // The probe is short; a timed-out probe finishes on its own, the file is only read.
            let probe = tokio::task::spawn_blocking(move || version_of(&file));
            match tokio::time::timeout(timeout, probe).await {
                Err(_) => Err(PingError::Timeout(timeout)),
                Ok(Err(join)) => Err(PingError::Failed(DbError::Server(join.to_string()))),
                Ok(Ok(Err(error))) => Err(PingError::Failed(error)),
                Ok(Ok(Ok(version))) => Ok(PingInfo {
                    server_version: format!("SQLite {version}"),
                    latency: start.elapsed(),
                    encryption: Encryption::Plain,
                }),
            }
        })
    }
}

/// The SQLite library's version, read from a read-only connection to the file (never created).
fn version_of(file: &std::path::Path) -> Result<String, DbError> {
    let conn = open::open(file, true, open::BUSY_WAIT)?;
    conn.query_row("SELECT sqlite_version()", [], |row| row.get::<_, String>(0)).map_err(open::server)
}
