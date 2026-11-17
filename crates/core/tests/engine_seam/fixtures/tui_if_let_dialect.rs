//! A UI module that branches on the dialect itself.
use datarig_core::sql::dialect::Dialect;

fn comment_marker(d: Dialect) -> &'static str {
    // Every other engine falls to the second branch, whatever its comments are.
    if let Dialect::Postgres = d { "--" } else { "#" }
}
