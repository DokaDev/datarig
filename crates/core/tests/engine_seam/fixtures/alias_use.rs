//! An alias of the enum does not hide its variants.
use super::dialect::Dialect as D;

fn backticks(d: D) -> bool {
    // MySQL quotes names with backticks.
    let mut yes = false;
    if let D::MySql(_) = d {
        yes = true;
    }
    yes
}
