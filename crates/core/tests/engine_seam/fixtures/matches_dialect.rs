//! `matches!` on a dialect: every other engine is silently "not PostgreSQL".
use super::dialect::Dialect;

fn dollar_quotes(d: Dialect) -> bool {
    // PostgreSQL reads dollar quotes.
    matches!(d, Dialect::Postgres)
}
