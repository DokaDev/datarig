//! The rule traits: every engine answers each rule itself.
use super::facts::{ServerFacts, Support};

pub trait CancelRules: Sync {
    fn server_cancel(&self, f: &ServerFacts) -> Support;
    /// A default answer holds for every engine that does not say otherwise.
    fn rollback_wait(&self) -> u64 {
        5
    }
}

pub trait TxRules: Sync {
    fn rollback_wait(&self) -> u64;
    /// A test helper: not a default answer, and not scanned.
    #[cfg(test)]
    fn probe(&self) -> bool {
        matches!(crate::sql::dialect::Dialect::default(), crate::sql::dialect::Dialect::Postgres)
    }
}
