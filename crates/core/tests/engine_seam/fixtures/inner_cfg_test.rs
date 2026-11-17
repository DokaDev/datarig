//! A module built only for the tests, by its own inner attribute: not scanned.
#![cfg(test)]
use datarig_core::sql::dialect::Dialect;

fn probe(d: Dialect, mariadb: bool) -> bool {
    mariadb || matches!(d, Dialect::Postgres)
}
