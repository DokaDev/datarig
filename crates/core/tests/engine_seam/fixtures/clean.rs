//! What the scan lets pass in a seam module: exhaustive matches with one arm per variant, and
//! catch-alls and comparisons on enums that carry no engine.
use super::dialect::Dialect as D;
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

fn comment(d: D) -> &'static str {
    match d {
        D::Postgres => "--",
        D::MySql(mode) if mode.ansi_quotes => "--",
        D::MySql(_) => "#",
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

fn fresh(d: D) -> D {
    let fresh = D::default();
    if d == fresh { d } else { fresh }
}

fn formatter() -> sqlformat::Dialect {
    sqlformat::Dialect::Generic
}
