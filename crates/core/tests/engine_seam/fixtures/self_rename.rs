//! The enum renamed through `self` in a grouped import is an alias too.
use super::dialect::Dialect::{self as D};

fn backticks(d: D) -> bool {
    // MySQL quotes names with backticks.
    matches!(d, D::MySql(_))
}
