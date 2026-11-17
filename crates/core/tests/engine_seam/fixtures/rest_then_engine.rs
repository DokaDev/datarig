//! Parts after a `..` are matched from the back: naming every engine there is exhaustive.
use super::dialect::Dialect;

fn pick(n: usize, d: Dialect) -> usize {
    match (n, d) {
        (.., Dialect::Postgres) => 1,
        (.., Dialect::MySql(_)) => 2,
    }
}
