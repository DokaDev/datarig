//! MySQL's session set-up.

use super::family::Family;
use datarig_core::engine::{EngineId, Fact, Version};

/// MySQL.
pub(crate) struct MySql;

impl Family for MySql {
    fn engine(&self) -> EngineId {
        EngineId::MySql
    }

    fn min_version(&self) -> Version {
        Version::new(8, 0, 0)
    }

    /// `transaction_read_only` on every version the driver works with (8.0 has no other name).
    fn read_only_var(&self, _: &Fact<Version>) -> &'static str {
        "transaction_read_only"
    }

    /// `transaction_isolation` on every version the driver works with.
    fn isolation_var(&self, _: &Fact<Version>) -> &'static str {
        "transaction_isolation"
    }

    /// `max_execution_time`, in milliseconds.
    fn statement_timeout(&self, ms: u64) -> String {
        format!("SESSION max_execution_time = {ms}")
    }

    fn super_read_only(&self) -> Option<&'static str> {
        Some("super_read_only")
    }
}
