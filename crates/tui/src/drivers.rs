//! Driver registry: maps a profile's `driver` name to an implementation. The core crate only
//! defines the [`Driver`] interface; this is where concrete driver crates are plugged in.
//! Tests replace the registry ([`crate::app::App::set_drivers`]) with fake drivers whose
//! sessions are plain channels.

use datarig_core::driver::Driver;
use datarig_core::engine::{self, EngineId, Expected};
use datarig_driver_mysql::MyDriver;
use datarig_driver_postgres::PgDriver;
use std::sync::Arc;

/// Looks a driver up by the profile's `driver` name.
pub type DriverLookup = Arc<dyn Fn(&str) -> Option<Arc<dyn Driver>> + Send + Sync>;

pub(crate) fn driver_for(name: &str) -> Option<Arc<dyn Driver>> {
    match engine::expected(name) {
        Expected::One(id) => Some(serving(id)),
        // A family's engines share one driver.
        Expected::Family(ids) => ids.first().copied().map(serving),
        Expected::Unknown(_) => None,
    }
}

/// The driver that serves engine `id`.
fn serving(id: EngineId) -> Arc<dyn Driver> {
    match id {
        EngineId::Postgres => Arc::new(PgDriver),
        EngineId::MySql | EngineId::MariaDb => Arc::new(MyDriver),
    }
}

#[cfg(test)]
mod tests;
