//! What each engine the driver serves says of a session's set-up ([`Family`]): the oldest
//! version the driver works with, and the names and forms of the session's settings, by the
//! server's version.

use datarig_core::driver::DbError;
use datarig_core::engine::{EngineId, Fact, Version};

/// An engine of the family, as the driver sets a session up on it.
pub(crate) trait Family: Sync {
    /// The engine.
    fn engine(&self) -> EngineId;

    /// The oldest version the driver works with.
    fn min_version(&self) -> Version;

    /// The session variable that makes the session's transactions read-only, on a server of
    /// version `v`.
    fn read_only_var(&self, v: &Fact<Version>) -> &'static str;

    /// The session variable of the session's transaction isolation level, on a server of
    /// version `v`.
    fn isolation_var(&self, v: &Fact<Version>) -> &'static str;

    /// The `SET` assignment that stops a `SELECT` after `ms` milliseconds.
    fn statement_timeout(&self, ms: u64) -> String;

    /// The server's own setting that refuses the writes of every session, a privileged one's
    /// too (asked for the message of a session that is not read-only); `None` on an engine that
    /// has none.
    fn super_read_only(&self) -> Option<&'static str>;
}

/// The family's rules of engine `e`.
pub(crate) fn family(e: EngineId) -> &'static dyn Family {
    match e {
        EngineId::MySql => &super::mysql::MySql,
        EngineId::MariaDb => &super::mariadb::MariaDb,
        // Never the engine of this driver's sessions: MySQL's, as the rest of the driver reads it.
        EngineId::Postgres => &super::mysql::MySql,
    }
}

/// The name a server of engine `e` is called by in a message.
fn name(e: EngineId) -> &'static str {
    match e {
        EngineId::MariaDb => "MariaDB",
        EngineId::MySql | EngineId::Postgres => "MySQL",
    }
}

/// `MySQL 8.4.6`, `MariaDB 11.4.2`: a server of `f`'s engine at version `v`.
pub(crate) fn label(f: &dyn Family, v: Version) -> String {
    format!("{} {}.{}.{}", name(f.engine()), v.major, v.minor, v.patch)
}

/// Refused when a server of `f`'s engine at version `v` is older than the driver supports
/// (0.0.0: it says no version the client can read).
pub(crate) fn check(f: &dyn Family, v: Version) -> Result<(), DbError> {
    let (min, name) = (f.min_version(), name(f.engine()));
    if v >= min {
        return Ok(());
    }
    let server = if v == Version::new(0, 0, 0) { format!("{name} (unknown version)") } else { label(f, v) };
    Err(DbError::VersionUnsupported { server, needed: format!("{name} {}.{}", min.major, min.minor) })
}
