//! An item that tests use is still product code when its cfg also holds without `test`.
use super::dialect::Dialect;

/// Built for the tests, and for any build with the feature on.
#[cfg(all(unix, any(test, feature = "extra")))]
fn dollar_quotes(d: Dialect) -> bool {
    matches!(d, Dialect::Postgres)
}
