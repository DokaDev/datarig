//! An alias declared inside a macro body is reported too.
wrap! {
    use crate::sql::dialect::Dialect as D;

    fn check(d: D) -> bool {
        matches!(d, D::Postgres)
    }
}
