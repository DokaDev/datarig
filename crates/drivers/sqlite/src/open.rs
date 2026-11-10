//! Opening a profile's database file: its path, the open mode, and the checks a connection
//! starts with (the file is a database, and the engine refuses `ATTACH` and `DETACH`).

use datarig_core::driver::DbError;
use datarig_core::profile::ConnectionConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a statement waits for another connection's lock before SQLite gives up with
/// "database is locked", when the profile sets no lock wait (`ConnectOptions::lock_wait`).
pub(crate) const BUSY_WAIT: Duration = Duration::from_secs(5);

/// The database file of a profile: its `path`, which must be absolute (a relative one would
/// depend on the directory datarig was started in).
pub(crate) fn file_of(cfg: &ConnectionConfig) -> Result<PathBuf, DbError> {
    let path = cfg.path.as_deref().map(str::trim).filter(|p| !p.is_empty()).ok_or(DbError::NoDatabase)?;
    let file = PathBuf::from(path);
    if !file.is_absolute() {
        return Err(DbError::Server(format!("the SQLite file must be an absolute path: {path}").into()));
    }
    Ok(file)
}

/// Opens `file` for one connection: read-only, or read-write, never created; `wait` is the lock
/// wait. Fails when the file is missing or is not a database.
pub(crate) fn open(file: &Path, read_only: bool, wait: Duration) -> Result<Connection, DbError> {
    let flags = if read_only { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE };
    let mut conn = Connection::open_with_flags(file, flags).map_err(server)?;
    conn.busy_timeout(wait).map_err(server)?;
    conn.authorizer(Some(refuse_attach)).map_err(server)?;
    if read_only {
        // A second layer behind the read-only open: the engine refuses a write on this connection.
        conn.pragma_update(None, "query_only", true).map_err(server)?;
    }
    // A file that is not a database fails here, not at the first statement.
    conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(())).map_err(server)?;
    Ok(conn)
}

/// Refuses `ATTACH` and `DETACH` (one profile is one file), whatever the statement says.
fn refuse_attach(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

/// An engine error as the driver's error.
pub(crate) fn server(error: rusqlite::Error) -> DbError {
    DbError::Server(error.to_string().into())
}
