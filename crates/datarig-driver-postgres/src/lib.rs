//! PostgreSQL driver on tokio-postgres.
//!
//! * One connection per session, by role (`SessionRole`): a *query* session per tab (user
//!   statements, portals, cancel) and one *meta* session per profile (tree + catalog), so
//!   browsing stays responsive while a long statement runs.
//! * Row-returning statements are executed through a portal inside the user's block or a
//!   transaction of their own and fetched `page_size` rows at a time with `Execute(max_rows)`;
//!   the first page is pipelined with its `BEGIN`, Bind and Describe into one round trip
//!   (`Transaction::bind_first_page` of the vendored tokio-postgres), each next page is one
//!   (`query_portal`). The user's SQL is never rewritten.
//! * Cancellation uses the protocol-level CancelRequest (`CancelToken::cancel_query`, or
//!   `cancel_query_raw` through the session's dialer: `route`).
//! * A session with a dialer (`ConnectOptions::dialer`, an SSH tunnel) reaches the server over
//!   a stream the dialer opens (`route`).

mod connect;
mod link;
mod meta;
mod query;
mod route;
mod values;

pub use connect::PgDriver;

/// The name of the savepoint counts and the allowlist's question run under, for the
/// integration tests that check none is left. Not an API.
#[doc(hidden)]
pub use query::count_savepoint;

/// The metadata session's catalog statements, for the integration tests' `EXPLAIN`s. Not an
/// API.
#[doc(hidden)]
pub mod catalog_sql {
    pub use crate::meta::structure::SQL as TABLE_STRUCTURE;
    pub use crate::meta::{BEGIN_READ, SCHEMA_OBJECTS};
}
