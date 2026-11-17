//! Comparing with a wrapped engine variant is a test of that variant too.
use super::dialect::Dialect;

fn picked_postgres(selected: Option<Dialect>) -> bool {
    selected == Some(Dialect::Postgres)
}

fn first_on_postgres(n: usize, d: Dialect) -> bool {
    (n, d) == (0, Dialect::Postgres)
}
