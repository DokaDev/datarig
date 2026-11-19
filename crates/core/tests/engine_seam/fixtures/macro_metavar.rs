//! A macro body with metavariables does not parse as Rust: an engine variant in it counts as a
//! branch on it.
use super::dialect::Dialect;

macro_rules! is_postgres {
    ($d:expr) => {
        matches!($d, Dialect::Postgres)
    };
}

macro_rules! is_postgres_too {
    ($d:expr) => {{
        type D = Dialect;
        let d = $d;
        matches!(d, D::Postgres)
    }};
}

macro_rules! postgres {
    () => {
        Dialect::Postgres
    };
}

fn on_postgres(d: Dialect) -> bool {
    matches!(d, postgres!())
}

impl Dialect {
    fn is_postgres(&self) -> bool {
        macro_rules! postgres_here {
            () => {
                Self::Postgres
            };
        }
        matches!(self, postgres_here!())
    }
}
