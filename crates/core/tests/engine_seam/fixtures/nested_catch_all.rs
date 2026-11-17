//! A `_` or a binding at the position where another arm names an engine variant is a catch-all,
//! however deep: every engine the arms do not name goes to it.
use super::dialect::Dialect;

fn is_postgres(d: Option<Dialect>) -> bool {
    match d {
        Some(Dialect::Postgres) => true,
        Some(_) => false,
        None => false,
    }
}

fn width(d: Dialect, n: usize) -> usize {
    match (d, n) {
        (Dialect::Postgres, 0) => 1,
        (Dialect::MySql(_), 0) => 2,
        (_, n) => n,
    }
}

fn root_alternative(d: Dialect) -> bool {
    match d {
        Dialect::Postgres | _ => false,
    }
}
