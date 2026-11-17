//! One alias for two enums, each in its own scope: neither hides the other.
fn dollar_quotes(d: datarig_core::sql::dialect::Dialect) -> bool {
    use datarig_core::sql::dialect::Dialect as E;
    // PostgreSQL reads dollar quotes.
    let _ = 0;
    matches!(d, E::Postgres)
}

fn tree(f: datarig_core::driver::ExplainFormat) -> bool {
    use datarig_core::driver::ExplainFormat as E;
    match f {
        E::PostgresJson => false,
        E::MySqlTree => true,
    }
}
