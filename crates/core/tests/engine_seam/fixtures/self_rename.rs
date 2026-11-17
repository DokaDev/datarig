//! The enum renamed through `self` in a grouped import is still the enum.
use super::dialect::Dialect::{self as D};

fn backticks(d: D) -> bool {
    // MySQL quotes names with backticks.
    matches!(d, D::MySql(_))
}
