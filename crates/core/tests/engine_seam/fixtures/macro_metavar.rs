//! A macro body with metavariables does not parse as Rust: an engine variant in it counts as a
//! branch on it.
use super::dialect::Dialect;

macro_rules! is_postgres {
    ($d:expr) => {
        matches!($d, Dialect::Postgres)
    };
}
