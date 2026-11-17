//! What the scan lets pass in a seam module: exhaustive matches with one arm per variant, and
//! catch-alls and comparisons on enums that carry no engine.
use super::dialect::Dialect;
use crate::driver::ExplainFormat;

impl ExplainFormat {
    /// Whether the plan is text.
    pub fn is_text(self) -> bool {
        match self {
            Self::PostgresJson => false,
            Self::MySqlTree => true,
        }
    }
}

fn comment(d: Dialect) -> &'static str {
    match d {
        Dialect::Postgres => "--",
        Dialect::MySql(mode) if mode.ansi_quotes => "--",
        Dialect::MySql(_) => "#",
    }
}

enum Shape {
    Line,
    Bar,
    Pie,
}

fn round(s: Shape) -> bool {
    match s {
        Shape::Pie => true,
        _ => false,
    }
}

fn same(a: &Shape, b: &Shape) -> bool {
    matches!(a, Shape::Line) && a == b
}

fn fresh(d: Dialect) -> Dialect {
    let fresh = Dialect::default();
    if d == fresh { d } else { fresh }
}

fn formatter() -> sqlformat::Dialect {
    sqlformat::Dialect::Generic
}

/// Built only for the tests: not scanned.
#[cfg(all(unix, any(test, all(test, feature = "extra"))))]
fn in_tests_only(d: Dialect) -> bool {
    matches!(d, Dialect::Postgres)
}

/// Test-only statements, arms and fields inside product code: not scanned.
fn checked(d: Dialect) -> u8 {
    #[cfg(test)]
    {
        assert!(matches!(d, Dialect::Postgres));
    }
    #[cfg(test)]
    if d == Dialect::Postgres {
        return 0;
    }
    match d {
        #[cfg(test)]
        _ if matches!(d, Dialect::Postgres) => 0,
        _ => 1,
    }
}

/// Test-only fields and parameters: not scanned.
struct Probe {
    #[cfg(test)]
    mysql: bool,
    #[cfg(test)]
    mariadb: bool,
}

fn probe(#[cfg(test)] pg: bool) {}

trait Probed {
    /// A test helper: not scanned.
    #[cfg(test)]
    fn probe(d: Dialect) -> bool {
        matches!(d, Dialect::Postgres)
    }
}

/// A comparison whose only engine variant is test-only: not a test of it.
fn one(n: usize) -> bool {
    n == {
        #[cfg(test)]
        {
            let _ = Dialect::Postgres;
        }
        1
    }
}
