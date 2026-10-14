//! Driver registry: maps a profile's `driver` name to an implementation. The core crate only
//! defines the [`Driver`] interface; this is where concrete driver crates are plugged in.
//! Tests replace the registry ([`crate::app::App::set_drivers`]) with fake drivers whose
//! sessions are plain channels.

use datarig_core::driver::Driver;
use datarig_driver_mysql::MyDriver;
use datarig_driver_postgres::PgDriver;
use std::sync::Arc;

/// Looks a driver up by the profile's `driver` name.
pub type DriverLookup = Arc<dyn Fn(&str) -> Option<Arc<dyn Driver>> + Send + Sync>;

pub(crate) fn driver_for(name: &str) -> Option<Arc<dyn Driver>> {
    match name.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" | "pg" => Some(Arc::new(PgDriver)),
        // MariaDB speaks the same protocol: a best effort.
        "mysql" | "mariadb" => Some(Arc::new(MyDriver)),
        _ => None,
    }
}
