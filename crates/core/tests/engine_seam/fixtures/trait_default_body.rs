//! The rule traits: every engine answers each rule itself.
use super::facts::{ServerFacts, Support};

pub trait CancelRules: Sync {
    fn server_cancel(&self, f: &ServerFacts) -> Support;
    /// A default answer holds for every engine that does not say otherwise.
    fn rollback_wait(&self) -> u64 {
        5
    }
}
