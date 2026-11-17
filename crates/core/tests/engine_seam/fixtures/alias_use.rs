//! Another name for an engine enum is itself reported: it would hide the enum from the scan.
use super::dialect::Dialect as D;

fn backticks(d: D) -> bool {
    // MySQL quotes names with backticks.
    let mut yes = false;
    if let D::MySql(_) = d {
        yes = true;
    }
    yes
}
