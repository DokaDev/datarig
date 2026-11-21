//! MariaDB's session set-up.

use super::family::Family;
use datarig_core::engine::{EngineId, Fact, Version};

/// MariaDB.
pub(crate) struct MariaDb;

/// The first version that names the session's transaction settings `transaction_read_only` and
/// `transaction_isolation`; an older one has only `tx_read_only` and `tx_isolation`, which later
/// versions keep as deprecated aliases.
const TRANSACTION_NAMES: Version = Version::new(11, 1, 0);

/// The name of a transaction setting on a server of version `v`: `new` from
/// [`TRANSACTION_NAMES`] on, else `old`, also when the version is not known: the old names are
/// the ones every version the driver works with takes.
fn by_version(v: &Fact<Version>, new: &'static str, old: &'static str) -> &'static str {
    match v {
        Fact::Known(v) if *v >= TRANSACTION_NAMES => new,
        Fact::Known(_) | Fact::No(_) | Fact::Unknown(_) => old,
    }
}

impl Family for MariaDb {
    fn engine(&self) -> EngineId {
        EngineId::MariaDb
    }

    /// A best effort.
    fn min_version(&self) -> Version {
        Version::new(10, 6, 0)
    }

    fn read_only_var(&self, v: &Fact<Version>) -> &'static str {
        by_version(v, "transaction_read_only", "tx_read_only")
    }

    fn isolation_var(&self, v: &Fact<Version>) -> &'static str {
        by_version(v, "transaction_isolation", "tx_isolation")
    }

    /// `max_statement_time`, in seconds.
    fn statement_timeout(&self, ms: u64) -> String {
        format!("SESSION max_statement_time = {}", ms as f64 / 1000.0)
    }

    /// None: MariaDB has no `super_read_only`.
    fn super_read_only(&self) -> Option<&'static str> {
        None
    }
}
