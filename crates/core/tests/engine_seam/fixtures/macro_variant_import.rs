//! A variant import inside a macro-generated function is reported like any other.
macro_rules! check {
    () => {
        fn check(d: Dialect) -> bool {
            use crate::sql::dialect::Dialect::*;
            matches!(d, Postgres)
        }
    };
}
