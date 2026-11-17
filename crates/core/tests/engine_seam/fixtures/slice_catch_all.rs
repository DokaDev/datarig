//! A wildcard beside an engine variant inside a slice pattern is a catch-all too.
use super::dialect::Dialect;

fn first_is_postgres(ds: &[Dialect]) -> u8 {
    match ds {
        [Dialect::Postgres, ..] => 1,
        _ => 0,
    }
}

fn first_engine(ds: &[Dialect]) -> u8 {
    match ds {
        [Dialect::Postgres, ..] => 1,
        [_, ..] => 2,
        [] => 0,
    }
}
