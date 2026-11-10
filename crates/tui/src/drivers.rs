//! Driver registry: maps a profile's `driver` name to an implementation. The core crate only
//! defines the [`Driver`] interface; this is where concrete driver crates are plugged in.
//! Tests replace the registry ([`crate::app::App::set_drivers`]) with fake drivers whose
//! sessions are plain channels.

use datarig_core::driver::Driver;
use datarig_core::profile::drivers::canonical;
use datarig_driver_mysql::MyDriver;
use datarig_driver_postgres::PgDriver;
use datarig_driver_sqlite::SqliteDriver;
use std::sync::Arc;

/// Looks a driver up by the profile's `driver` name.
pub type DriverLookup = Arc<dyn Fn(&str) -> Option<Arc<dyn Driver>> + Send + Sync>;

/// The driver for a profile's `driver` name (any spelling `profile::drivers` knows). A name the
/// app knows but has no implementation for yet gives `None`.
pub(crate) fn driver_for(name: &str) -> Option<Arc<dyn Driver>> {
    match canonical(name)? {
        "postgres" => Some(Arc::new(PgDriver)),
        // MariaDB speaks the same protocol: a best effort.
        "mysql" => Some(Arc::new(MyDriver)),
        "sqlite" => Some(Arc::new(SqliteDriver)),
        _ => None,
    }
}
